use seex::{
    EvidenceCompleteness, MetricKey, ObjectiveDirection, ObjectiveMetric, ProjectId, RunId, Step,
};

#[test]
fn facade_exports_common_product_types_without_storage_inputs() {
    let project_id = ProjectId::from_string("project-1");
    let run_id = RunId::from_string("run-1");
    let objective = ObjectiveMetric {
        metric_key: MetricKey::from_string("loss"),
        direction: ObjectiveDirection::Minimize,
    };

    assert_eq!(project_id.as_str(), "project-1");
    assert_eq!(run_id.as_str(), "run-1");
    assert_eq!(objective.metric_key.as_str(), "loss");
    assert_eq!(Step::new(7).value(), 7);
    assert!(EvidenceCompleteness::Complete < EvidenceCompleteness::Unavailable);
}
