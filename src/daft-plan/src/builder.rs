use std::sync::Arc;

use crate::logical_plan::LogicalPlan;
use crate::planner::PhysicalPlanner;
use crate::source_info::{FileInfo, PyFileFormatConfig, SourceInfo};
use crate::{ops, PartitionScheme, PartitionSpec};

#[cfg(feature = "python")]
use {
    daft_core::python::schema::PySchema,
    daft_dsl::python::PyExpr,
    pyo3::{prelude::*, types::PyList},
};

#[cfg_attr(feature = "python", pyclass)]
#[derive(Debug)]
pub struct LogicalPlanBuilder {
    plan: Arc<LogicalPlan>,
}

impl LogicalPlanBuilder {
    pub fn new(plan: Arc<LogicalPlan>) -> Self {
        Self { plan }
    }
}

#[cfg(feature = "python")]
#[pymethods]
impl LogicalPlanBuilder {
    #[staticmethod]
    pub fn table_scan(
        file_paths: Vec<String>,
        schema: &PySchema,
        file_format_config: PyFileFormatConfig,
    ) -> PyResult<LogicalPlanBuilder> {
        let num_partitions = file_paths.len();
        let source_info = SourceInfo::new(
            schema.schema.clone(),
            FileInfo::new(file_paths, None, None, None),
            file_format_config.into(),
        );
        let partition_spec = PartitionSpec::new(PartitionScheme::Unknown, num_partitions, None);
        let logical_plan: LogicalPlan = ops::Source::new(
            schema.schema.clone(),
            source_info.into(),
            partition_spec.into(),
        ).into();        
        let logical_plan_builder = LogicalPlanBuilder::new(logical_plan.into());
        Ok(logical_plan_builder)
    }

    pub fn filter(&self, predicate: &PyExpr) -> PyResult<LogicalPlanBuilder> {
        let logical_plan: LogicalPlan = ops::Filter::new(
            predicate.expr.clone(),
            self.plan.clone(),
        ).into();
        let logical_plan_builder = LogicalPlanBuilder::new(logical_plan.into());
        Ok(logical_plan_builder)
    }

    pub fn limit(&self, limit: i64) -> PyResult<LogicalPlanBuilder> {
        let logical_plan: LogicalPlan = ops::Limit::new(limit, self.plan.clone()).into();
        let logical_plan_builder = LogicalPlanBuilder::new(logical_plan.into());
        Ok(logical_plan_builder)
    }

    pub fn aggregate(&self, aggregates: &PyList) -> PyResult<LogicalPlanBuilder> {
        todo!()
    }

    pub fn schema(&self) -> PyResult<PySchema> {
        Ok(self.plan.schema().into())
    }

    pub fn partition_spec(&self) -> PyResult<PartitionSpec> {
        Ok(self.plan.partition_spec().as_ref().clone())
    }

    pub fn to_partition_tasks(&self) -> PyResult<PyObject> {
        let planner = PhysicalPlanner {};
        let physical_plan = planner.plan(self.plan.as_ref())?;
        Python::with_gil(|py| physical_plan.to_partition_tasks(py))
    }

    pub fn repr_ascii(&self) -> PyResult<String> {
        Ok(self.plan.repr_ascii())
    }
}
