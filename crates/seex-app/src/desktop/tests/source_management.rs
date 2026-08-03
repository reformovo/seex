use gpui::{Modifiers, TestAppContext};
use seex::Project;

use super::super::test_support::open_viewer;
use super::*;
use crate::data::SourcePreflight;
use crate::domain::SourceAlias;

#[gpui::test]
fn source_confirmation_requires_a_valid_non_empty_selection(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let timestamp = "2026-01-01T00:00:00Z"
        .parse()
        .expect("test timestamp should parse");
    let projects = ["one", "two"]
        .into_iter()
        .map(|id| Project {
            project_id: ProjectId::from_string(id),
            name: id.to_owned(),
            created_at: timestamp,
        })
        .collect();
    let (window, mut cx) = open_viewer(cx, None);
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.begin(
                    SourcePreflight {
                        root_path: root.path().to_owned(),
                        projects,
                    },
                    SourceAlias::new("research").expect("test alias should be valid"),
                    window,
                    cx,
                );
            });
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    assert!(cx.debug_bounds("source-confirmation").is_some());

    for selector in ["source-project:one", "source-project:two"] {
        let row = cx
            .debug_bounds(selector)
            .expect("Project choice should render");
        cx.simulate_click(row.center(), Modifiers::default());
    }
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    let confirm = cx
        .debug_bounds("confirm-source")
        .expect("confirm control should render");
    cx.simulate_click(confirm.center(), Modifiers::default());
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    assert!(cx.debug_bounds("source-confirmation").is_some());

    let row = cx
        .debug_bounds("source-project:one")
        .expect("Project choice should remain rendered");
    cx.simulate_click(row.center(), Modifiers::default());
    let confirm = cx
        .debug_bounds("confirm-source")
        .expect("confirm control should remain rendered");
    cx.simulate_click(confirm.center(), Modifiers::default());
    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .is_open())
            .expect("viewer should remain open")
    );
}
