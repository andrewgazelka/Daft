import multiprocessing
from concurrent.futures import wait

import pytest

import daft
from daft import DataType, ResourceRequest
from daft.context import get_context
from daft.execution.execution_step import StatefulUDFProject
from daft.expressions import ExpressionsProjection
from daft.runners.partitioning import PartialPartitionMetadata
from daft.runners.pyrunner import PyActorPool, PyRunner
from daft.table import MicroPartition


@daft.udf(return_dtype=DataType.int64())
class MyStatefulUDF:
    def __init__(self):
        self.state = 0

    def __call__(self, x):
        self.state += 1
        return [i + self.state for i in x.to_pylist()]


def test_pyactor_pool():
    projection = ExpressionsProjection([MyStatefulUDF(daft.col("x"))])
    pool = PyActorPool("my-pool", 1, ResourceRequest(num_cpus=1), projection)
    initial_partition = MicroPartition.from_pydict({"x": [1, 1, 1]})
    ppm = PartialPartitionMetadata(num_rows=None, size_bytes=None)
    instr = StatefulUDFProject(projection=projection)

    pool_id = pool.setup()
    assert pool_id == "my-pool"

    result = pool.submit(
        instruction_stack=[instr],
        partitions=[initial_partition],
        final_metadata=[ppm],
    )
    done, _ = wait([result], timeout=None)
    result_data = list(done)[0].result()[0]
    assert result_data.partition().to_pydict() == {"x": [2, 2, 2]}

    result = pool.submit(
        instruction_stack=[instr],
        partitions=[initial_partition],
        final_metadata=[ppm],
    )
    done, _ = wait([result], timeout=None)
    result_data = list(done)[0].result()[0]
    assert result_data.partition().to_pydict() == {"x": [3, 3, 3]}

    result = pool.submit(
        instruction_stack=[instr],
        partitions=[initial_partition],
        final_metadata=[ppm],
    )
    done, _ = wait([result], timeout=None)
    result_data = list(done)[0].result()[0]
    assert result_data.partition().to_pydict() == {"x": [4, 4, 4]}


@pytest.mark.skipif(get_context().runner_config.name != "py", reason="Test can only be run on PyRunner")
def test_pyactor_pool_not_enough_resources():
    cpu_count = multiprocessing.cpu_count()
    projection = ExpressionsProjection([MyStatefulUDF(daft.col("x"))])

    runner = get_context().runner()
    assert isinstance(runner, PyRunner)

    with pytest.raises(RuntimeError, match=f"Requested {float(cpu_count + 1)} CPUs but found only"):
        with runner.actor_pool_context("my-pool", ResourceRequest(num_cpus=1), cpu_count + 1, projection) as _:
            pass