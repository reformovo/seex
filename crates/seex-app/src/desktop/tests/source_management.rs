use gpui::{Modifiers, TestAppContext};
use seex::{Client, Project, RunOptions};

use super::super::test_support::{open_viewer, wait_for_viewer};
use super::*;
use crate::data::SourcePreflight;
use crate::domain::SourceAlias;

#[gpui::test]
fn source_confirmation_requires_a_valid_non_empty_selection(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let scope = tempfile::tempdir().expect("test scope should be created");
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
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
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
    let config_path = scope.path().join(".seex/config.toml");
    for _ in 0..100 {
        cx.run_until_parked();
        if config_path.exists() {
            break;
        }
    }
    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .is_open())
            .expect("viewer should remain open")
    );
    let saved = std::fs::read_to_string(config_path).expect("Source config should be saved");
    let saved = saved
        .parse::<toml_edit::DocumentMut>()
        .expect("Source config should remain valid TOML");
    let projects = saved["sources"]["research"]["projects"]
        .as_array()
        .expect("Source Project allowlist should be an array");
    assert_eq!(projects.len(), 1);
    assert_eq!(
        projects.get(0).and_then(toml_edit::Value::as_str),
        Some("one")
    );
}

#[gpui::test]
fn failed_source_config_write_does_not_switch_live_sources(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let source = tempfile::tempdir().expect("test Source should be created");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    let config_path = scope.path().join(".seex/config.toml");
    std::fs::create_dir_all(
        config_path
            .parent()
            .expect("test config should have a parent"),
    )
    .expect("test config directory should be created");
    std::fs::write(&config_path, "schema_version = 1\n# external\n")
        .expect("external edit should be written");

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: false,
                    alias: SourceAlias::new("research").expect("test alias should be valid"),
                    root_path: source.path().to_owned(),
                    projects: vec![ProjectId::from_string("project")],
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    cx.run_until_parked();

    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .sources
                .is_empty())
            .expect("viewer should remain open")
    );
    assert_eq!(
        std::fs::read_to_string(config_path).expect("external config should remain readable"),
        "schema_version = 1\n# external\n"
    );
}

#[gpui::test]
fn managing_projects_updates_allowlist_and_clears_unimported_references(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let source = tempfile::tempdir().expect("test Source should be created");
    let client = Client::builder(source.path())
        .open()
        .expect("test client should open");
    for project in ["one", "two"] {
        client
            .start_run(RunOptions::new(project).id(project).name(project))
            .expect("test Run should start")
            .finish()
            .expect("test Run should finish");
    }
    client.shutdown().expect("test client should shut down");
    let alias = SourceAlias::new("research").expect("test alias should be valid");
    let one = ProjectId::from_string("one");
    let two = ProjectId::from_string("two");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: false,
                    alias: alias.clone(),
                    root_path: source.path().to_owned(),
                    projects: vec![one.clone(), two.clone()],
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .first()
            .is_some_and(|source| source.project_allowlist.len() == 2)
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.session.update(cx, |session, _| {
                session.views.pin_project(ProjectRef::new(
                    DataSourceId::from_alias(&alias),
                    one.clone(),
                ));
            });
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: true,
                    alias: alias.clone(),
                    root_path: source.path().to_owned(),
                    projects: vec![two.clone()],
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .first()
            .is_some_and(|source| source.project_allowlist == [two.clone()])
    });

    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            assert_eq!(snapshot.sources[0].project_allowlist, [two.clone()]);
            assert!(snapshot.views.pinned_projects().is_empty());
        })
        .expect("viewer should remain open");

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: true,
                    alias,
                    root_path: source.path().to_owned(),
                    projects: Vec::new(),
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).sources.is_empty()
    });
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .sources
                .is_empty())
            .expect("viewer should remain open")
    );
}
