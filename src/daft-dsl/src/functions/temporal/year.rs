use common_error::{DaftError, DaftResult};
use daft_core::prelude::*;

use super::super::FunctionEvaluator;
use crate::{functions::FunctionExpr, ExprRef};

pub(super) struct YearEvaluator {}

impl FunctionEvaluator for YearEvaluator {
    fn fn_name(&self) -> &'static str {
        "year"
    }

    fn to_field(&self, inputs: &[ExprRef], schema: &Schema, _: &FunctionExpr) -> DaftResult<Field> {
        match inputs {
            [input] => match input.to_field(schema) {
                Ok(field) if field.dtype.is_temporal() => {
                    Ok(Field::new(field.name, DataType::Int32))
                }
                Ok(field) => Err(DaftError::TypeError(format!(
                    "Expected input to year to be temporal, got {}",
                    field.dtype
                ))),
                Err(e) => Err(e),
            },
            _ => Err(DaftError::SchemaMismatch(format!(
                "Expected 1 input arg, got {}",
                inputs.len()
            ))),
        }
    }

    fn evaluate(&self, inputs: &[Series], _: &FunctionExpr) -> DaftResult<Series> {
        match inputs {
            [input] => input.dt_year(),
            _ => Err(DaftError::ValueError(format!(
                "Expected 1 input arg, got {}",
                inputs.len()
            ))),
        }
    }
}
