use arrow2::array::PrimitiveArray;
use common_error::{DaftError, DaftResult};
use logical::Decimal128Array;

use crate::{
    array::{
        ops::{DaftHllMergeAggable, GroupIndices},
        ListArray,
    },
    count_mode::CountMode,
    datatypes::*,
    series::{IntoSeries, Series},
    with_match_physical_daft_types,
};

impl Series {
    pub fn count(&self, groups: Option<&GroupIndices>, mode: CountMode) -> DaftResult<Series> {
        use crate::array::ops::DaftCountAggable;
        let s = self.as_physical()?;
        with_match_physical_daft_types!(s.data_type(), |$T| {
            match groups {
                Some(groups) => Ok(DaftCountAggable::grouped_count(&s.downcast::<<$T as DaftDataType>::ArrayType>()?, groups, mode)?.into_series()),
                None => Ok(DaftCountAggable::count(&s.downcast::<<$T as DaftDataType>::ArrayType>()?, mode)?.into_series())
            }
        })
    }

    pub fn sum(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        use crate::{array::ops::DaftSumAggable, datatypes::DataType::*};

        match self.data_type() {
            // intX -> int64 (in line with numpy)
            Int8 | Int16 | Int32 | Int64 => {
                let casted = self.cast(&Int64)?;
                match groups {
                    Some(groups) => {
                        Ok(DaftSumAggable::grouped_sum(&casted.i64()?, groups)?.into_series())
                    }
                    None => Ok(DaftSumAggable::sum(&casted.i64()?)?.into_series()),
                }
            }
            // uintX -> uint64 (in line with numpy)
            UInt8 | UInt16 | UInt32 | UInt64 => {
                let casted = self.cast(&UInt64)?;
                match groups {
                    Some(groups) => {
                        Ok(DaftSumAggable::grouped_sum(&casted.u64()?, groups)?.into_series())
                    }
                    None => Ok(DaftSumAggable::sum(&casted.u64()?)?.into_series()),
                }
            }
            // floatX -> floatX (in line with numpy)
            Float32 => match groups {
                Some(groups) => Ok(DaftSumAggable::grouped_sum(
                    &self.downcast::<Float32Array>()?,
                    groups,
                )?
                .into_series()),
                None => Ok(DaftSumAggable::sum(&self.downcast::<Float32Array>()?)?.into_series()),
            },
            Float64 => match groups {
                Some(groups) => Ok(DaftSumAggable::grouped_sum(
                    &self.downcast::<Float64Array>()?,
                    groups,
                )?
                .into_series()),
                None => Ok(DaftSumAggable::sum(&self.downcast::<Float64Array>()?)?.into_series()),
            },
            Decimal128(_, _) => match groups {
                Some(groups) => Ok(Decimal128Array::new(
                    Field {
                        dtype: try_sum_supertype(self.data_type())?,
                        ..self.field().clone()
                    },
                    DaftSumAggable::grouped_sum(
                        &self.as_physical()?.downcast::<Int128Array>()?,
                        groups,
                    )?,
                )
                .into_series()),
                None => Ok(Decimal128Array::new(
                    Field {
                        dtype: try_sum_supertype(self.data_type())?,
                        ..self.field().clone()
                    },
                    DaftSumAggable::sum(&self.as_physical()?.downcast::<Int128Array>()?)?,
                )
                .into_series()),
            },
            other => Err(DaftError::TypeError(format!(
                "Numeric sum is not implemented for type {}",
                other
            ))),
        }
    }

    pub fn approx_sketch(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        use crate::{array::ops::DaftApproxSketchAggable, datatypes::DataType::*};

        // Upcast all numeric types to float64 and compute approx_sketch.
        match self.data_type() {
            dt if dt.is_numeric() => {
                let casted = self.cast(&Float64)?;
                match groups {
                    Some(groups) => Ok(DaftApproxSketchAggable::grouped_approx_sketch(
                        &casted.f64()?,
                        groups,
                    )?
                    .into_series()),
                    None => {
                        Ok(DaftApproxSketchAggable::approx_sketch(&casted.f64()?)?.into_series())
                    }
                }
            }
            other => Err(DaftError::TypeError(format!(
                "Approx sketch is not implemented for type {}",
                other
            ))),
        }
    }

    pub fn merge_sketch(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        use crate::{array::ops::DaftMergeSketchAggable, datatypes::DataType::*};

        match self.data_type() {
            Struct(_) => match groups {
                Some(groups) => Ok(DaftMergeSketchAggable::grouped_merge_sketch(
                    &self.struct_()?,
                    groups,
                )?
                .into_series()),
                None => Ok(DaftMergeSketchAggable::merge_sketch(&self.struct_()?)?.into_series()),
            },
            other => Err(DaftError::TypeError(format!(
                "Merge sketch is not implemented for type {}",
                other
            ))),
        }
    }

    pub fn hll_merge(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        let downcasted_self = self.downcast::<FixedSizeBinaryArray>()?;
        let series = match groups {
            Some(groups) => downcasted_self.grouped_hll_merge(groups),
            None => downcasted_self.hll_merge(),
        }?
        .into_series();
        Ok(series)
    }

    pub fn mean(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        use crate::{array::ops::DaftMeanAggable, datatypes::DataType::*};

        // Upcast all numeric types to float64 and use f64 mean kernel.
        match self.data_type() {
            dt if dt.is_numeric() => {
                let casted = self.cast(&Float64)?;
                match groups {
                    Some(groups) => {
                        Ok(DaftMeanAggable::grouped_mean(&casted.f64()?, groups)?.into_series())
                    }
                    None => Ok(DaftMeanAggable::mean(&casted.f64()?)?.into_series()),
                }
            }
            other => Err(DaftError::TypeError(format!(
                "Numeric mean is not implemented for type {}",
                other
            ))),
        }
    }

    pub fn min(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        self.inner.min(groups)
    }

    pub fn max(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        self.inner.max(groups)
    }

    pub fn any_value(
        &self,
        groups: Option<&GroupIndices>,
        ignore_nulls: bool,
    ) -> DaftResult<Series> {
        let indices = match groups {
            Some(groups) => {
                if self.data_type().is_null() {
                    Box::new(PrimitiveArray::new_null(
                        arrow2::datatypes::DataType::UInt64,
                        groups.len(),
                    ))
                } else if ignore_nulls && let Some(validity) = self.validity() {
                    Box::new(PrimitiveArray::from_trusted_len_iter(groups.iter().map(
                        |g| g.iter().find(|i| validity.get_bit(**i as usize)).copied(),
                    )))
                } else {
                    Box::new(PrimitiveArray::from_trusted_len_iter(
                        groups.iter().map(|g| g.first().cloned()),
                    ))
                }
            }
            None => {
                let idx = if self.data_type().is_null() || self.is_empty() {
                    None
                } else if ignore_nulls && let Some(validity) = self.validity() {
                    validity.iter().position(|v| v).map(|i| i as u64)
                } else {
                    Some(0)
                };

                Box::new(PrimitiveArray::from([idx]))
            }
        };

        self.take(&Series::from_arrow(
            Field::new("", DataType::UInt64).into(),
            indices,
        )?)
    }

    pub fn agg_list(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        self.inner.agg_list(groups)
    }

    pub fn agg_concat(&self, groups: Option<&GroupIndices>) -> DaftResult<Series> {
        use crate::array::ops::DaftConcatAggable;
        match self.data_type() {
            DataType::List(..) => {
                let downcasted = self.downcast::<ListArray>()?;
                match groups {
                    Some(groups) => {
                        Ok(DaftConcatAggable::grouped_concat(downcasted, groups)?.into_series())
                    }
                    None => Ok(DaftConcatAggable::concat(downcasted)?.into_series()),
                }
            }
            #[cfg(feature = "python")]
            DataType::Python => {
                let downcasted = self.downcast::<PythonArray>()?;
                match groups {
                    Some(groups) => {
                        Ok(DaftConcatAggable::grouped_concat(downcasted, groups)?.into_series())
                    }
                    None => Ok(DaftConcatAggable::concat(downcasted)?.into_series()),
                }
            }
            _ => Err(DaftError::TypeError(format!(
                "concat aggregation is only valid for List or Python types, got {}",
                self.data_type()
            ))),
        }
    }
}
