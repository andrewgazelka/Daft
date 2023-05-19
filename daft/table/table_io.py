from __future__ import annotations

import contextlib
import pathlib
from collections.abc import Generator
from typing import IO, Union
from urllib.parse import urlparse
from uuid import uuid4

import fsspec
import pyarrow as pa
from pyarrow import csv as pacsv
from pyarrow import dataset as pads
from pyarrow import fs as pafs
from pyarrow import json as pajson
from pyarrow import parquet as papq

from daft.expressions import ExpressionsProjection
from daft.filesystem import get_filesystem_from_path
from daft.runners.partitioning import (
    vPartitionParseCSVOptions,
    vPartitionReadOptions,
    vPartitionSchemaInferenceOptions,
)
from daft.table import Table

FileInput = Union[pathlib.Path, str, IO[bytes]]


@contextlib.contextmanager
def _get_file(
    file: FileInput,
    fs: fsspec.AbstractFileSystem | None,
) -> Generator[FileInput, None, None]:
    """Helper method to return an appropriate file handle

    1. If `fs` is not None, we fall-back onto the provided fsspec FileSystem and return an fsspec file handle
    2. If `file` is a pathlib, we stringify it
    3. If `file` is a string, we leave it unmodified
    """
    if isinstance(file, pathlib.Path):
        file = str(file)

    if isinstance(file, str):
        # Use provided fsspec filesystem, slow but necessary for backward-compatibility
        if fs is not None:
            with fs.open(file, compression="infer") as f:
                yield f
        # Corner-case to handle `http` filepaths using fsspec because PyArrow cannot handle it
        elif urlparse(file).scheme in {"http", "https"}:
            fsspec_fs = get_filesystem_from_path(file)
            with fsspec_fs.open(file, compression="infer") as f:
                yield f
        # Safely yield a string path, which can be correctly interpreted by PyArrow filesystem
        else:
            yield file
    else:
        yield file


@contextlib.contextmanager
def _ensure_pyarrow_files_for_parquet(file: FileInput) -> Generator[FileInput, None, None]:
    # NOTE: Before PyArrow 10.0.0, the Parquet metadata methods cannot read s3 URLs, so we open
    # any strings as URLs manually here if we encounter them. Otherwise, this function is a no-op.
    #
    # See: https://issues.apache.org/jira/browse/ARROW-16719
    if isinstance(file, str):
        fs, path = pafs.FileSystem.from_uri(file)
        with fs.open_input_file(path) as f:
            yield f
    else:
        yield f


def read_json(
    file: FileInput,
    fs: fsspec.AbstractFileSystem | None = None,
    read_options: vPartitionReadOptions = vPartitionReadOptions(),
) -> Table:
    """Reads a Table from a JSON file

    Args:
        file (str | IO): either a file-like object or a string file path (potentially prefixed with a protocol such as "s3://")
        fs (fsspec.AbstractFileSystem): fsspec FileSystem to use for reading data.
            By default, Daft will automatically construct a FileSystem instance internally.
        read_options (vPartitionReadOptions, optional): Options for reading the file

    Returns:
        Table: Parsed Table from JSON
    """
    with _get_file(file, fs) as f:
        table = pajson.read_json(f)

    if read_options.column_names is not None:
        table = table.select(read_options.column_names)

    # TODO(jay): Can't limit number of rows with current PyArrow filesystem so we have to shave it off after the read
    if read_options.num_rows is not None:
        table = table[: read_options.num_rows]

    return Table.from_arrow(table)


def read_parquet(
    file: FileInput,
    fs: fsspec.AbstractFileSystem | None = None,
    read_options: vPartitionReadOptions = vPartitionReadOptions(),
) -> Table:
    """Reads a Table from a Parquet file

    Args:
        file (str | IO): either a file-like object or a string file path (potentially prefixed with a protocol such as "s3://")
        fs (fsspec.AbstractFileSystem): fsspec FileSystem to use for reading data.
            By default, Daft will automatically construct a FileSystem instance internally.
        read_options (vPartitionReadOptions, optional): Options for reading the file

    Returns:
        Table: Parsed Table from Parquet
    """
    with _get_file(file, fs) as f:
        with _ensure_pyarrow_files_for_parquet(f) as f:
            pqf = papq.ParquetFile(f)
            # If no rows required, we manually construct an empty table with the right schema
            if read_options.num_rows == 0:
                arrow_schema = pqf.metadata.schema.to_arrow_schema()
                table = pa.Table.from_arrays(
                    [pa.array([], type=field.type) for field in arrow_schema], schema=arrow_schema
                )
            elif read_options.num_rows is not None:
                # Read the file by rowgroup.
                tables = []
                rows_read = 0
                for i in range(pqf.metadata.num_row_groups):
                    tables.append(pqf.read_row_group(i, columns=read_options.column_names))
                    rows_read += len(tables[i])
                    if rows_read >= read_options.num_rows:
                        break
                table = pa.concat_tables(tables)
                table = table.slice(length=read_options.num_rows)
            else:
                table = papq.read_table(
                    f,
                    columns=read_options.column_names,
                )

    return Table.from_arrow(table)


def read_csv(
    file: FileInput,
    fs: fsspec.AbstractFileSystem | None = None,
    csv_options: vPartitionParseCSVOptions = vPartitionParseCSVOptions(),
    schema_options: vPartitionSchemaInferenceOptions = vPartitionSchemaInferenceOptions(),
    read_options: vPartitionReadOptions = vPartitionReadOptions(),
) -> Table:
    """Reads a Table from a CSV file

    Args:
        file (str | IO): either a file-like object or a string file path (potentially prefixed with a protocol such as "s3://")
        fs (fsspec.AbstractFileSystem): fsspec FileSystem to use for reading data.
            By default, Daft will automatically construct a FileSystem instance internally.
        csv_options (vPartitionParseCSVOptions, optional): CSV-specific configs to apply when reading the file
        schema_options (vPartitionSchemaInferenceOptions, optional): configs to apply when inferring schema from the file
        read_options (vPartitionReadOptions, optional): Options for reading the file

    Returns:
        Table: Parsed Table from CSV
    """
    # Use provided CSV column names, or None if nothing provided
    full_column_names = schema_options.full_schema_column_names()

    # Have PyArrow generate the column names if the CSV has no header and no column names were provided
    pyarrow_autogenerate_column_names = (not csv_options.has_headers) and (full_column_names is None)

    # Have Pyarrow skip the header row if column names were provided, and a header exists in the CSV
    skip_header_row = full_column_names is not None and csv_options.has_headers
    pyarrow_skip_rows_after_names = (1 if skip_header_row else 0) + csv_options.skip_rows_after_header

    with _get_file(file, fs) as f:
        table = pacsv.read_csv(
            f,
            parse_options=pacsv.ParseOptions(
                delimiter=csv_options.delimiter,
            ),
            # skip_rows applied, header row is read if column_names is not None, skip_rows_after_names is applied
            read_options=pacsv.ReadOptions(
                autogenerate_column_names=pyarrow_autogenerate_column_names,
                column_names=full_column_names,
                skip_rows_after_names=pyarrow_skip_rows_after_names,
                skip_rows=csv_options.skip_rows_before_header,
            ),
            convert_options=pacsv.ConvertOptions(include_columns=read_options.column_names),
        )

    # TODO(jay): Can't limit number of rows with current PyArrow filesystem so we have to shave it off after the read
    if read_options.num_rows is not None:
        table = table[: read_options.num_rows]

    return Table.from_arrow(table)


def write_csv(
    table: Table,
    path: str | pathlib.Path,
    compression: str | None = None,
    partition_cols: ExpressionsProjection | None = None,
) -> list[str]:
    return _to_file(
        table=table,
        file_format="csv",
        path=path,
        partition_cols=partition_cols,
        compression=compression,
    )


def write_parquet(
    table: Table,
    path: str | pathlib.Path,
    compression: str | None = None,
    partition_cols: ExpressionsProjection | None = None,
) -> list[str]:
    return _to_file(
        table=table,
        file_format="parquet",
        path=path,
        partition_cols=partition_cols,
        compression=compression,
    )


def _to_file(
    table: Table,
    file_format: str,
    path: str | pathlib.Path,
    partition_cols: ExpressionsProjection | None = None,
    compression: str | None = None,
) -> list[str]:
    arrow_table = table.to_arrow()

    partitioning = [e.name() for e in (partition_cols or [])]
    if partitioning:
        # In partition cols, downcast large_string to string,
        # since pyarrow.dataset.write_dataset breaks for large_string partitioning columns.
        downcasted_schema = pa.schema(
            [
                pa.field(
                    name=field.name,
                    type=pa.string(),
                    nullable=field.nullable,
                    metadata=field.metadata,
                )
                if field.name in partitioning and field.type == pa.large_string()
                else field
                for field in arrow_table.schema
            ]
        )
        arrow_table = arrow_table.cast(downcasted_schema)

    if file_format == "parquet":
        format = pads.ParquetFileFormat()
        opts = format.make_write_options(compression=compression)
    elif file_format == "csv":
        format = pads.CsvFileFormat()
        opts = None
        assert compression is None
    else:
        raise ValueError(f"Unsupported file format {file_format}")

    visited_paths = []

    def file_visitor(written_file):
        visited_paths.append(written_file.path)

    pads.write_dataset(
        arrow_table,
        base_dir=path,
        basename_template=str(uuid4()) + "-{i}." + format.default_extname,
        format=format,
        partitioning=partitioning,
        file_options=opts,
        file_visitor=file_visitor,
        use_threads=False,
        existing_data_behavior="overwrite_or_ignore",
    )

    return visited_paths
