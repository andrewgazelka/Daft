use std::{collections::HashMap, sync::Arc};

use common_error::DaftResult;
use common_treenode::{ConcreteTreeNode, TreeNode};
use daft_core::schema::Schema;
use daft_dsl::Expr;
use daft_micropartition::MicroPartition;
use daft_physical_plan::{
    Concat, Filter, HashAggregate, HashJoin, InMemoryScan, Limit, LocalPhysicalPlan, PhysicalScan,
    Project, Sort, UnGroupedAggregate,
};
use daft_plan::populate_aggregation_stages;

use crate::{
    channel::MultiSender,
    intermediate_ops::{
        aggregate::AggregateOperator,
        filter::FilterOperator,
        intermediate_op::{run_intermediate_op, IntermediateOperator},
        project::ProjectOperator,
    },
    sinks::{
        aggregate::AggregateSink,
        concat::ConcatSink,
        hash_join::HashJoinSink,
        limit::LimitSink,
        sink::{run_double_input_sink, run_single_input_sink, DoubleInputSink, SingleInputSink},
        sort::SortSink,
    },
    sources::{
        in_memory::InMemorySource,
        scan_task::ScanTaskSource,
        source::{run_source, Source},
    },
};

pub enum PipelineNode {
    Source {
        source: Box<dyn Source>,
    },
    IntermediateOp {
        intermediate_op: Arc<dyn IntermediateOperator>,
        children: Vec<PipelineNode>,
    },
    SingleInputSink {
        sink: Box<dyn SingleInputSink>,
        child: Box<PipelineNode>,
    },
    DoubleInputSink {
        sink: Box<dyn DoubleInputSink>,
        left_child: Box<PipelineNode>,
        right_child: Box<PipelineNode>,
    },
}

impl PipelineNode {
    pub fn start(self, sender: MultiSender) {
        match self {
            PipelineNode::Source { source } => {
                run_source(source, sender);
            }
            PipelineNode::IntermediateOp {
                intermediate_op,
                mut children,
            } => {
                assert!(children.len() == 1, "we can only handle 1 child for intermediate ops right now: {}", children.len());
                let child = children.pop().expect("exactly 1 child");
                let sender = run_intermediate_op(intermediate_op, sender);
                child.start(sender);
            }
            PipelineNode::SingleInputSink { sink, child } => {
                let sender = run_single_input_sink(sink, sender);
                child.start(sender);
            }
            PipelineNode::DoubleInputSink {
                sink,
                left_child,
                right_child,
            } => {
                let (left_sender, right_sender) = run_double_input_sink(sink, sender);
                left_child.start(left_sender);
                right_child.start(right_sender);
            }
        }
    }
}

pub fn physical_plan_to_pipeline(
    physical_plan: &LocalPhysicalPlan,
    psets: &HashMap<String, Vec<Arc<MicroPartition>>>,
) -> DaftResult<PipelineNode> {
    Ok(match physical_plan {
        LocalPhysicalPlan::PhysicalScan(PhysicalScan { scan_tasks, .. }) => {
            let scan_task_source = ScanTaskSource::new(scan_tasks.clone());
            PipelineNode::Source {
                source: Box::new(scan_task_source),
            }
        }
        LocalPhysicalPlan::InMemoryScan(InMemoryScan { info, .. }) => {
            let partitions = psets.get(&info.cache_key).expect("Cache key not found");
            let in_memory_source = InMemorySource::new(partitions.clone());
            PipelineNode::Source {
                source: Box::new(in_memory_source),
            }
        }
        LocalPhysicalPlan::Project(Project {
            input, projection, ..
        }) => {
            let proj_op = ProjectOperator::new(projection.clone());
            let child_node = physical_plan_to_pipeline(input, psets)?;
            PipelineNode::IntermediateOp {
                intermediate_op: Arc::new(proj_op),
                children: vec![child_node],
            }
        }
        LocalPhysicalPlan::Filter(Filter {
            input, predicate, ..
        }) => {
            let filter_op = FilterOperator::new(predicate.clone());
            let child_node = physical_plan_to_pipeline(input, psets)?;
            PipelineNode::IntermediateOp {
                intermediate_op: Arc::new(filter_op),
                children: vec![child_node],
            }
        }
        LocalPhysicalPlan::Limit(Limit {
            input, num_rows, ..
        }) => {
            let sink = LimitSink::new(*num_rows as usize);
            let child_node = physical_plan_to_pipeline(input, psets)?;
            PipelineNode::SingleInputSink {
                sink: Box::new(sink),
                child: Box::new(child_node),
            }
        }
        LocalPhysicalPlan::Concat(Concat { input, other, .. }) => {
            let sink = ConcatSink::new();
            let left_child = physical_plan_to_pipeline(input, psets)?;
            let right_child = physical_plan_to_pipeline(other, psets)?;
            PipelineNode::DoubleInputSink {
                sink: Box::new(sink),
                left_child: Box::new(left_child),
                right_child: Box::new(right_child),
            }
        }
        LocalPhysicalPlan::UnGroupedAggregate(UnGroupedAggregate {
            input,
            aggregations,
            schema,
            ..
        }) => {
            let (first_stage_aggs, second_stage_aggs, final_exprs) =
                populate_aggregation_stages(aggregations, schema, &[]);
            let first_stage_agg_op = AggregateOperator::new(
                first_stage_aggs
                    .values()
                    .cloned()
                    .map(|e| Arc::new(Expr::Agg(e.clone())))
                    .collect(),
                vec![],
            );
            let second_stage_agg_sink = AggregateSink::new(
                second_stage_aggs
                    .values()
                    .cloned()
                    .map(|e| Arc::new(Expr::Agg(e.clone())))
                    .collect(),
                vec![],
            );
            let final_stage_project = ProjectOperator::new(final_exprs);

            let child_node = physical_plan_to_pipeline(input, psets)?;
            let intermediate_agg_op_node = PipelineNode::IntermediateOp {
                intermediate_op: Arc::new(first_stage_agg_op),
                children: vec![child_node],
            };

            let sink_node = PipelineNode::SingleInputSink {
                sink: Box::new(second_stage_agg_sink),
                child: Box::new(intermediate_agg_op_node),
            };

            PipelineNode::IntermediateOp {
                intermediate_op: Arc::new(final_stage_project),
                children: vec![sink_node],
            }
        }
        LocalPhysicalPlan::HashAggregate(HashAggregate {
            input,
            aggregations,
            group_by,
            schema,
            ..
        }) => {
            let (first_stage_aggs, second_stage_aggs, final_exprs) =
                populate_aggregation_stages(aggregations, schema, group_by);
            let first_stage_agg_op = AggregateOperator::new(
                first_stage_aggs
                    .values()
                    .cloned()
                    .map(|e| Arc::new(Expr::Agg(e.clone())))
                    .collect(),
                group_by.clone(),
            );
            let second_stage_agg_sink = AggregateSink::new(
                second_stage_aggs
                    .values()
                    .cloned()
                    .map(|e| Arc::new(Expr::Agg(e.clone())))
                    .collect(),
                group_by.clone(),
            );
            let final_stage_project = ProjectOperator::new(final_exprs);

            let child_node = physical_plan_to_pipeline(input, psets)?;
            let intermediate_agg_op_node = PipelineNode::IntermediateOp {
                intermediate_op: Arc::new(first_stage_agg_op),
                children: vec![child_node],
            };

            let sink_node = PipelineNode::SingleInputSink {
                sink: Box::new(second_stage_agg_sink),
                child: Box::new(intermediate_agg_op_node),
            };

            PipelineNode::IntermediateOp {
                intermediate_op: Arc::new(final_stage_project),
                children: vec![sink_node],
            }
        }
        LocalPhysicalPlan::Sort(Sort {
            input,
            sort_by,
            descending,
            ..
        }) => {
            let sort_sink = SortSink::new(sort_by.clone(), descending.clone());
            let child_node = physical_plan_to_pipeline(input, psets)?;
            PipelineNode::SingleInputSink {
                sink: Box::new(sort_sink),
                child: Box::new(child_node),
            }
        }
        LocalPhysicalPlan::HashJoin(HashJoin {
            left,
            right,
            left_on,
            right_on,
            join_type,
            schema,
        }) => {
            let left_schema = left.schema();
            let right_schema = right.schema();
            let left_node = physical_plan_to_pipeline(left, psets)?;
            let right_node = physical_plan_to_pipeline(right, psets)?;
            let sink = HashJoinSink::new(
                left_on.clone(),
                right_on.clone(),
                *join_type,
                left_schema,
                right_schema,
            )?;
            PipelineNode::DoubleInputSink {
                sink: Box::new(sink),
                left_child: Box::new(left_node),
                right_child: Box::new(right_node),
            }
        }
        _ => {
            unimplemented!("Physical plan not supported: {}", physical_plan.name());
        }
    })
}

// impl TreeNode for PipelineNode {
//     fn apply_children<F: FnMut(&Self) -> DaftResult<common_treenode::TreeNodeRecursion>>(
//             &self,
//             f: F,
//         ) -> DaftResult<common_treenode::TreeNodeRecursion> {
        
//     }
    
//     // fn children(&self) -> Vec<&Self> {
//     //     use PipelineNode::*;
//     //     match self.as_ref() {
//     //         Source { .. } => vec![],
//     //         IntermediateOp { child, ..} | SingleInputSink { child, ..} => vec![child],
//     //         DoubleInputSink {left_child, right_child,.. } => vec![left_child, right_child],
//     //     }
//     // }
//     // fn take_children(self) -> (Self, Vec<Self>) {
//     //     use PipelineNode::*;
//     //     match self {
//     //         Source { source } => vec![],
//     //         IntermediateOp { child, ..} | SingleInputSink { child, ..} => vec![child],
//     //         DoubleInputSink {left_child, right_child,.. } => vec![left_child, right_child],
//     //     } 
//     // }
// }