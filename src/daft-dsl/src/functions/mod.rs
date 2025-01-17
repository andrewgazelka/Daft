pub mod map;
pub mod numeric;
pub mod partitioning;
pub mod scalar;
pub mod sketch;
pub mod struct_;
pub mod temporal;
pub mod utf8;

use std::{
    fmt::{Display, Formatter, Result, Write},
    hash::Hash,
};

use common_error::DaftResult;
use daft_core::prelude::*;
pub use scalar::*;
use serde::{Deserialize, Serialize};

use self::{
    map::MapExpr, numeric::NumericExpr, partitioning::PartitioningExpr, sketch::SketchExpr,
    struct_::StructExpr, temporal::TemporalExpr, utf8::Utf8Expr,
};
use crate::{Expr, ExprRef, Operator};

pub mod python;
use python::PythonUDF;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FunctionExpr {
    Numeric(NumericExpr),
    Utf8(Utf8Expr),
    Temporal(TemporalExpr),
    Map(MapExpr),
    Sketch(SketchExpr),
    Struct(StructExpr),
    Python(PythonUDF),
    Partitioning(PartitioningExpr),
}

pub trait FunctionEvaluator {
    fn fn_name(&self) -> &'static str;
    fn to_field(
        &self,
        inputs: &[ExprRef],
        schema: &Schema,
        expr: &FunctionExpr,
    ) -> DaftResult<Field>;
    fn evaluate(&self, inputs: &[Series], expr: &FunctionExpr) -> DaftResult<Series>;
}

impl FunctionExpr {
    #[inline]
    fn get_evaluator(&self) -> &dyn FunctionEvaluator {
        use FunctionExpr::*;
        match self {
            Numeric(expr) => expr.get_evaluator(),
            Utf8(expr) => expr.get_evaluator(),
            Temporal(expr) => expr.get_evaluator(),
            Map(expr) => expr.get_evaluator(),
            Sketch(expr) => expr.get_evaluator(),
            Struct(expr) => expr.get_evaluator(),
            Python(expr) => expr.get_evaluator(),
            Partitioning(expr) => expr.get_evaluator(),
        }
    }
}

impl Display for FunctionExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "{}", self.fn_name())
    }
}

impl FunctionEvaluator for FunctionExpr {
    fn fn_name(&self) -> &'static str {
        self.get_evaluator().fn_name()
    }

    fn to_field(
        &self,
        inputs: &[ExprRef],
        schema: &Schema,
        expr: &FunctionExpr,
    ) -> DaftResult<Field> {
        self.get_evaluator().to_field(inputs, schema, expr)
    }

    fn evaluate(&self, inputs: &[Series], expr: &FunctionExpr) -> DaftResult<Series> {
        self.get_evaluator().evaluate(inputs, expr)
    }
}

pub fn function_display(f: &mut Formatter, func: &FunctionExpr, inputs: &[ExprRef]) -> Result {
    write!(f, "{}(", func)?;
    for (i, input) in inputs.iter().enumerate() {
        if i != 0 {
            write!(f, ", ")?;
        }
        write!(f, "{input}")?;
    }
    write!(f, ")")?;
    Ok(())
}

pub fn function_display_without_formatter(
    func: &FunctionExpr,
    inputs: &[ExprRef],
) -> std::result::Result<String, std::fmt::Error> {
    let mut f = String::default();
    write!(&mut f, "{}(", func)?;
    for (i, input) in inputs.iter().enumerate() {
        if i != 0 {
            write!(&mut f, ", ")?;
        }
        write!(&mut f, "{input}")?;
    }
    write!(&mut f, ")")?;
    Ok(f)
}

pub fn binary_op_display_without_formatter(
    op: &Operator,
    left: &ExprRef,
    right: &ExprRef,
) -> std::result::Result<String, std::fmt::Error> {
    let mut f = String::default();
    let write_out_expr = |f: &mut String, input: &Expr| match input {
        Expr::Alias(e, _) => write!(f, "{e}"),
        Expr::BinaryOp { .. } => write!(f, "[{input}]"),
        _ => write!(f, "{input}"),
    };
    write_out_expr(&mut f, left)?;
    write!(&mut f, " {op} ")?;
    write_out_expr(&mut f, right)?;
    Ok(f)
}

pub fn function_semantic_id(func: &FunctionExpr, inputs: &[ExprRef], schema: &Schema) -> FieldID {
    let inputs = inputs
        .iter()
        .map(|expr| expr.semantic_id(schema).id.to_string())
        .collect::<Vec<String>>()
        .join(", ");
    // TODO: check for function idempotency here.
    FieldID::new(format!("Function_{func:?}({inputs})"))
}
