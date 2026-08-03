use gpui::{Modifiers, TestAppContext, point, px, size};
use seex::{Client, Project, RunId, RunOptions};

use super::super::test_support::{open_viewer, saved_workbench, wait_for_viewer};
use super::*;
use crate::data::SourcePreflight;
use crate::domain::SourceAlias;

#[gpui::test]
fn sources_control_opens_confirmation_before_path_prompt(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let (_window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");

    let import = cx
        .debug_bounds("sources")
        .expect("Sources control should render");
    cx.simulate_click(import.center(), Modifiers::default());
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");

    assert!(cx.debug_bounds("sources-dialog").is_some());
    assert!(cx.debug_bounds("add-sources").is_some());
}

#[gpui::test]
fn manage_and_workbench_dialogs_share_responsive_geometry(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let source = tempfile::tempdir().expect("test Source should be created");
    let timestamp = "2026-01-01T00:00:00Z"
        .parse()
        .expect("test timestamp should parse");
    let project_id = ProjectId::from_string("one");
    let alias = SourceAlias::new("research").expect("test alias should be valid");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.begin_manage(
                    SourcePreflight {
                        root_path: source.path().to_owned(),
                        projects: vec![Project {
                            project_id: project_id.clone(),
                            name: "Project".to_owned(),
                            created_at: timestamp,
                        }],
                    },
                    alias.clone(),
                    std::slice::from_ref(&project_id),
                    window,
                    cx,
                );
            });
        })
        .expect("viewer should remain open");
    cx.simulate_resize(size(px(600.), px(320.)));
    cx.run_until_parked();
    cx.refresh().expect("Manage dialog should render");

    let dialog = cx
        .debug_bounds("sources-dialog")
        .expect("Manage dialog should render");
    assert_eq!(dialog.size.width, px(568.));
    assert!(dialog.size.height <= px(288.));

    cx.simulate_resize(size(px(480.), px(600.)));
    cx.run_until_parked();
    cx.refresh().expect("narrow Sources dialog should render");
    let dialog = cx
        .debug_bounds("sources-dialog")
        .expect("narrow Sources dialog should render");
    let list = cx
        .debug_bounds("sources-list")
        .expect("narrow Source list should render");
    let detail = cx
        .debug_bounds("source-detail")
        .expect("narrow Source detail should render");
    assert_eq!(dialog.size.width, px(448.));
    assert!(list.bottom() <= detail.origin.y);

    cx.simulate_resize(size(px(600.), px(320.)));
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.begin_workbench(
                    WorkbenchImportPlan {
                        document: saved_workbench(
                            alias.clone(),
                            project_id.clone(),
                            Vec::new(),
                            "loss",
                        ),
                        alias_rewrites: vec![(alias.clone(), alias.clone())],
                        allowlist_additions: vec![(alias.clone(), vec![project_id.clone()])],
                    },
                    cx,
                );
            });
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    cx.refresh().expect("Workbench dialog should render");

    let dialog = cx
        .debug_bounds("workbench-import-dialog")
        .expect("Workbench dialog should render");
    assert_eq!(dialog.size.width, px(520.));
    assert!(dialog.size.height <= px(288.));
}

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
            name: "Same name".to_owned(),
            created_at: timestamp,
        })
        .collect();
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.begin_import(
                    Vec::new(),
                    vec![SourceAlias::new("research").expect("test alias should be valid")],
                    window,
                    cx,
                );
                let generation = management
                    .begin_source_preflight(root.path().to_owned(), cx)
                    .expect("import confirmation should accept a Source preflight");
                management.finish_source_preflight(
                    generation,
                    Ok((
                        SourcePreflight {
                            root_path: root.path().to_owned(),
                            projects,
                        },
                        SourceAlias::new("research").expect("test alias should be valid"),
                    )),
                    window,
                    cx,
                );
            });
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .active_alias_state(cx))
            .expect("viewer should remain open"),
        (Some("research".to_owned()), true)
    );
    assert!(cx.debug_bounds("sources-dialog").is_some());
    assert!(cx.debug_bounds("sources-body").is_some());
    assert!(cx.debug_bounds("source-item:research").is_some());
    assert!(cx.debug_bounds("source-detail").is_some());
    assert!(cx.debug_bounds("source-alias-input").is_some());
    assert!(cx.debug_bounds("source-alias-selection").is_some());
    assert!(cx.debug_bounds("source-alias-caret").is_some());
    assert!(cx.debug_bounds("source-alias-error").is_some());
    assert!(cx.debug_bounds("source-project-id:one").is_some());
    assert!(cx.debug_bounds("source-project-id:two").is_some());
    let project_id = cx
        .debug_bounds("source-project-id:one")
        .expect("Project ID should render");
    let checkbox = cx
        .debug_bounds("source-project-checkbox:one")
        .expect("Project checkbox should render");
    assert!(checkbox.origin.x > project_id.origin.x);
    let row = cx
        .debug_bounds("source-project:one")
        .expect("Project row should render");
    assert_eq!(checkbox.right(), row.right() - px(8.));
    assert_eq!(
        cx.debug_bounds("sources-dialog")
            .expect("Source dialog should render")
            .size
            .width,
        px(640.)
    );
    for selector in ["source-alias-input", "save-sources"] {
        assert_eq!(
            cx.debug_bounds(selector)
                .expect("dialog control should render")
                .size
                .height,
            px(28.)
        );
    }

    let select_all = cx
        .debug_bounds("select-all-projects")
        .expect("Select all control should render");
    cx.simulate_click(select_all.center(), Modifiers::default());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .is_open())
            .expect("viewer should remain open")
    );
    cx.refresh().expect("test window should refresh");
    let clear = cx
        .debug_bounds("clear-projects")
        .expect("Clear control should render");
    cx.simulate_click(clear.center(), Modifiers::default());
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    let alias = cx
        .debug_bounds("source-alias-input")
        .expect("Alias input should render");
    cx.simulate_click(
        point(alias.origin.x + px(9.), alias.center().y),
        Modifiers::default(),
    );
    cx.simulate_keystrokes("local");

    let confirm = cx
        .debug_bounds("save-sources")
        .expect("confirm control should render");
    cx.simulate_click(confirm.center(), Modifiers::default());
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    assert!(cx.debug_bounds("sources-dialog").is_some());

    let row = cx
        .debug_bounds("source-project:one")
        .expect("Project choice should render");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");
    let alias = cx
        .debug_bounds("source-alias-input")
        .expect("Alias input should remain rendered");
    cx.simulate_click(alias.center(), Modifiers::default());
    cx.simulate_keystrokes("enter");
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
    let projects = saved["sources"]["localresearch"]["projects"]
        .as_array()
        .expect("Source Project allowlist should be an array");
    assert_eq!(projects.len(), 1);
    assert_eq!(
        projects.get(0).and_then(toml_edit::Value::as_str),
        Some("one")
    );
}

#[gpui::test]
fn multiple_source_drafts_finish_out_of_order_and_save_together(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let roots = tempfile::tempdir().expect("Source roots should be created");
    let first = roots.path().join("first");
    let second = roots.path().join("second");
    std::fs::create_dir_all(&first).expect("first Source should be created");
    std::fs::create_dir_all(&second).expect("second Source should be created");
    let timestamp = "2026-01-01T00:00:00Z"
        .parse()
        .expect("test timestamp should parse");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.begin_sources(Vec::new(), Vec::new(), window, cx);
                let requests =
                    management.queue_source_paths(vec![first.clone(), second.clone()], cx);
                for request in requests.into_iter().rev() {
                    let project_id = ProjectId::from_string(
                        request
                            .root_path
                            .file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .expect("Source name should be UTF-8"),
                    );
                    management.finish_preflight(
                        request.clone(),
                        Ok(SourcePreflight {
                            root_path: request.root_path,
                            projects: vec![Project {
                                project_id: project_id.clone(),
                                name: project_id.as_str().to_owned(),
                                created_at: timestamp,
                            }],
                        }),
                        window,
                        cx,
                    );
                }
            });
        })
        .expect("viewer should remain open");
    cx.refresh().expect("Sources should render");

    for (source_selector, project_selector) in [
        ("source-item:first", "source-project:first"),
        ("source-item:second", "source-project:second"),
    ] {
        let source = cx
            .debug_bounds(source_selector)
            .expect("Source item should render");
        cx.simulate_click(source.center(), Modifiers::default());
        cx.run_until_parked();
        cx.refresh().expect("Source detail should refresh");
        let project = cx
            .debug_bounds(project_selector)
            .expect("Source Project should render");
        cx.simulate_click(project.center(), Modifiers::default());
    }
    let save = cx
        .debug_bounds("save-sources")
        .expect("Save Sources should render");
    cx.simulate_click(save.center(), Modifiers::default());

    let config_path = scope.path().join(".seex/config.toml");
    for _ in 0..100 {
        cx.run_until_parked();
        if config_path.exists() {
            break;
        }
    }
    let saved = std::fs::read_to_string(config_path)
        .expect("both Sources should be saved")
        .parse::<toml_edit::DocumentMut>()
        .expect("Source configuration should remain valid");
    assert_eq!(
        (
            saved["sources"]["first"]["projects"][0].as_str(),
            saved["sources"]["second"]["projects"][0].as_str(),
        ),
        (Some("first"), Some("second"))
    );
}

#[gpui::test]
fn sources_manager_reuses_existing_source_and_allows_new_projects(cx: &mut TestAppContext) {
    let (source, one, two) = source_with_two_projects();
    let scope = tempfile::tempdir().expect("test scope should be created");
    let config_path = scope.path().join(".seex/config.toml");
    std::fs::create_dir_all(config_path.parent().expect("config should have a parent"))
        .expect("config directory should be created");
    std::fs::write(
        &config_path,
        format!(
            "schema_version = 1\n[sources.research]\npath = {:?}\nprojects = ['one']\n\
             [sources.research-copy]\npath = {:?}\nprojects = ['one']\n",
            source.path().to_string_lossy(),
            source.path().to_string_lossy(),
        ),
    )
    .expect("legacy duplicate config should be written");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .any(|source| source.source_id.alias().as_str() == "research")
    });
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.open_source_import(window, cx);
        })
        .expect("viewer should remain open");
    cx.refresh().expect("Sources should render");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.select_source_alias("research", cx);
            });
        })
        .expect("viewer should remain open");
    for _ in 0..100 {
        cx.run_until_parked();
        cx.refresh().expect("Sources should refresh");
        if cx.debug_bounds("source-project:two").is_some() {
            break;
        }
    }
    cx.refresh().expect("Sources should render");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .active_source_state())
            .expect("viewer should remain open"),
        Some(("research".to_owned(), 2, None))
    );
    assert!(cx.debug_bounds("source-alias-input").is_none());
    assert!(cx.debug_bounds("source-alias-readonly").is_some());

    let save = cx
        .debug_bounds("save-sources")
        .expect("disabled Save control should render");
    cx.simulate_click(save.center(), Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .is_open())
            .expect("viewer should remain open")
    );

    let available = cx
        .debug_bounds("source-project:two")
        .expect("new Project should remain selectable");
    cx.simulate_click(available.center(), Modifiers::default());
    cx.simulate_click(save.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).sources.iter().any(|source| {
            source.source_id.alias().as_str() == "research"
                && source.project_allowlist == [one.clone(), two.clone()]
        })
    });
    let saved = std::fs::read_to_string(config_path)
        .expect("duplicate configuration should remain readable")
        .parse::<toml_edit::DocumentMut>()
        .expect("duplicate configuration should remain valid TOML");
    let project_count = |alias: &str| {
        saved["sources"][alias]["projects"]
            .as_array()
            .map(|projects| projects.len())
    };
    assert_eq!(
        (project_count("research"), project_count("research-copy")),
        (Some(2), Some(1))
    );
    assert!(saved["sources"].get("research-2").is_none());
}

#[gpui::test]
fn cancelled_confirmation_ignores_late_source_preflight(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let source = tempfile::tempdir().expect("test Source should be created");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    let generation = window
        .update(&mut cx, |viewer, window, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.begin_import(Vec::new(), Vec::new(), window, cx);
                management
                    .begin_source_preflight(source.path().to_owned(), cx)
                    .expect("open import confirmation should accept preflight")
            })
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.source_management.update(cx, |management, cx| {
                management.finish_source_preflight(
                    generation,
                    Ok((
                        SourcePreflight {
                            root_path: source.path().to_owned(),
                            projects: Vec::new(),
                        },
                        SourceAlias::new("late").expect("test alias should be valid"),
                    )),
                    window,
                    cx,
                );
            });
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    cx.refresh().expect("test window should refresh");

    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .is_open())
            .expect("viewer should remain open")
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
            .read_with(&cx, |viewer, cx| {
                !viewer
                    .session_snapshot(cx)
                    .sources
                    .iter()
                    .any(|source| source.source_id.alias().as_str() == "research")
            })
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
            .iter()
            .any(|source| source.source_id.alias() == &alias && source.project_allowlist.len() == 2)
    });
    let source_id = DataSourceId::from_alias(&alias);
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.session.update(cx, |session, _| {
                session
                    .views
                    .pin_project(ProjectRef::new(source_id.clone(), one.clone()));
            });
            viewer.manage_source_projects(source_id.clone(), window, cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.source_management.read(cx).is_open()
    });
    cx.refresh().expect("Manage Projects should render");
    let research = cx
        .debug_bounds("source-item:research")
        .expect("research Source should render");
    cx.simulate_click(research.center(), Modifiers::default());
    for _ in 0..100 {
        cx.run_until_parked();
        cx.refresh().expect("Sources should refresh");
        if cx.debug_bounds("source-project:one").is_some() {
            break;
        }
    }
    assert!(cx.debug_bounds("source-detail").is_some());
    assert!(cx.debug_bounds("source-alias-readonly").is_some());
    assert!(cx.debug_bounds("source-alias-input").is_none());
    let one_row = cx
        .debug_bounds("source-project:one")
        .expect("Project one should render");
    cx.simulate_click(one_row.center(), Modifiers::default());
    let save = cx
        .debug_bounds("save-sources")
        .expect("Manage Save should render");
    cx.simulate_click(save.center(), Modifiers::default());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .session_snapshot(cx)
                    .sources
                    .iter()
                    .find(|source| source.source_id.alias() == &alias)
                    .expect("research Source should remain")
                    .project_allowlist
                    .clone()
            })
            .expect("viewer should remain open"),
        [one.clone(), two.clone()]
    );

    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.manage_source_projects(source_id, window, cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.source_management.read(cx).is_open()
    });
    cx.refresh().expect("Manage Projects should render");
    let research = cx
        .debug_bounds("source-item:research")
        .expect("research Source should render");
    cx.simulate_click(research.center(), Modifiers::default());
    for _ in 0..100 {
        cx.run_until_parked();
        cx.refresh().expect("Sources should refresh");
        if cx.debug_bounds("source-project:one").is_some() {
            break;
        }
    }
    let one_row = cx
        .debug_bounds("source-project:one")
        .expect("Project one should render");
    cx.simulate_click(one_row.center(), Modifiers::default());
    let save = cx
        .debug_bounds("save-sources")
        .expect("Manage Save should render");
    cx.simulate_click(save.center(), Modifiers::default());
    cx.simulate_prompt_answer("Remove");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).sources.iter().any(|source| {
            source.source_id.alias() == &alias && source.project_allowlist == [two.clone()]
        })
    });

    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            let source = snapshot
                .sources
                .iter()
                .find(|source| source.source_id.alias() == &alias)
                .expect("research Source should remain");
            assert_eq!(source.project_allowlist, std::slice::from_ref(&two));
            assert!(snapshot.views.pinned_projects().is_empty());
        })
        .expect("viewer should remain open");

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: true,
                    alias: alias.clone(),
                    root_path: source.path().to_owned(),
                    projects: Vec::new(),
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        !viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .any(|source| source.source_id.alias() == &alias)
    });
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                !viewer
                    .session_snapshot(cx)
                    .sources
                    .iter()
                    .any(|source| source.source_id.alias() == &alias)
            })
            .expect("viewer should remain open")
    );
}

#[gpui::test]
fn reload_sources_rejects_invalid_candidates_before_switching_live_state(cx: &mut TestAppContext) {
    let scope = tempfile::tempdir().expect("test scope should be created");
    let first = tempfile::tempdir().expect("first Source should be created");
    let second = tempfile::tempdir().expect("second Source should be created");
    for (root, project) in [(first.path(), "one"), (second.path(), "two")] {
        let client = Client::builder(root)
            .open()
            .expect("test client should open");
        client
            .start_run(RunOptions::new(project).id(project).name(project))
            .expect("test Run should start")
            .finish()
            .expect("test Run should finish");
        client.shutdown().expect("test client should shut down");
    }
    let config_path = scope.path().join(".seex/config.toml");
    std::fs::create_dir_all(config_path.parent().expect("config should have a parent"))
        .expect("config directory should be created");
    let source_document = |path: &std::path::Path, project: &str| {
        format!(
            "schema_version = 1\n[sources.research]\npath = {:?}\nprojects = [{project:?}]\n",
            path.to_string_lossy()
        )
    };
    std::fs::write(&config_path, source_document(first.path(), "one"))
        .expect("initial config should be written");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .find(|source| source.source_id.alias().as_str() == "research")
            .is_some_and(|source| source.root_path == first.path())
    });

    std::fs::write(&config_path, "schema_version = 99\n")
        .expect("invalid external edit should be written");
    window
        .update(&mut cx, |viewer, _, cx| viewer.reload_sources(cx))
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session.read(cx).transient_error.is_some()
    });
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.session_snapshot(cx).sources.iter().any(|source| {
                    source.source_id.alias().as_str() == "research"
                        && source.root_path == first.path()
                })
            })
            .expect("viewer should remain open")
    );

    std::fs::write(&config_path, source_document(second.path(), "two"))
        .expect("valid external edit should be written");
    window
        .update(&mut cx, |viewer, _, cx| viewer.reload_sources(cx))
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .find(|source| source.source_id.alias().as_str() == "research")
            .is_some_and(|source| {
                source.root_path == second.path()
                    && source.project_allowlist == [ProjectId::from_string("two")]
            })
    });
}

fn source_with_two_projects() -> (tempfile::TempDir, ProjectId, ProjectId) {
    let root = tempfile::tempdir().expect("test Source should be created");
    let client = Client::builder(root.path())
        .open()
        .expect("test client should open");
    let one = ProjectId::from_string("one");
    let two = ProjectId::from_string("two");
    for project in [&one, &two] {
        client
            .start_run(
                RunOptions::new(project.as_str())
                    .id(project.as_str())
                    .name("available"),
            )
            .expect("test Run should start")
            .finish()
            .expect("test Run should finish");
    }
    client.shutdown().expect("test client should shut down");
    (root, one, two)
}

#[gpui::test]
fn workbench_import_confirms_rewrites_and_replaces_blocked_live_state(cx: &mut TestAppContext) {
    let (source, one, two) = source_with_two_projects();
    let scope = tempfile::tempdir().expect("test scope should be created");
    let external = scope.path().join("external.toml");
    let workbench_path = scope.path().join(".seex/workbench.toml");
    let local_alias = SourceAlias::new("research").expect("local alias should be valid");
    let external_alias = SourceAlias::new("portable").expect("external alias should be valid");
    let mut imported = saved_workbench(
        external_alias,
        two.clone(),
        vec![RunId::from_string("missing")],
        "loss",
    );
    imported.views[0].name = "Imported".to_owned();
    imported
        .save(&external)
        .expect("external workbench should save");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: false,
                    alias: local_alias.clone(),
                    root_path: source.path().to_owned(),
                    projects: vec![one.clone()],
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .find(|source| source.source_id.alias() == &local_alias)
            .is_some_and(|source| source.project_allowlist == [one.clone()])
    });
    std::fs::write(&workbench_path, "schema_version = 99\n")
        .expect("invalid local workbench should be written");
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.session.update(cx, |session, _| {
                session.workbench_path = Some(workbench_path.clone());
                session.autosave_blocked = true;
            });
            viewer.preflight_workbench_path(external.clone(), window, cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.source_management.read(cx).is_open()
    });
    cx.refresh().expect("confirmation should render");
    assert_eq!(
        cx.debug_bounds("workbench-import-dialog")
            .expect("Workbench dialog should render")
            .size
            .width,
        px(520.)
    );
    window
        .read_with(&cx, |viewer, _| {
            let plan = &viewer
                .pending_workbench_import
                .as_ref()
                .expect("pending import should remain available")
                .plan;
            assert_eq!(plan.alias_rewrites.len(), 1);
            assert_eq!(plan.allowlist_additions.len(), 1);
        })
        .expect("viewer should remain open");
    let confirm = cx
        .debug_bounds("confirm-workbench-import")
        .expect("import confirmation should render");
    cx.simulate_click(confirm.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        let session = viewer.session.read(cx);
        session.views.active().name == "Imported"
            && !session.autosave_blocked
            && session
                .sources
                .sources()
                .find(|source| source.source_id.alias() == &local_alias)
                .is_some_and(|source| source.project_allowlist == [one.clone(), two.clone()])
    });

    let saved = WorkbenchDocument::load(&workbench_path)
        .expect("imported workbench should decode")
        .expect("imported workbench should exist");
    assert_eq!(saved.views[0].runs[0].source_alias, local_alias);
    assert_eq!(saved.views[0].runs[0].run_id.as_str(), "missing");
}

#[gpui::test]
fn workbench_import_configures_a_new_source_only_after_confirmation(cx: &mut TestAppContext) {
    let (source, _, project_id) = source_with_two_projects();
    let scope = tempfile::tempdir().expect("test scope should be created");
    let external = scope.path().join("external.toml");
    let workbench_path = scope.path().join(".seex/workbench.toml");
    let config_path = scope.path().join(".seex/config.toml");
    let alias = SourceAlias::new("portable").expect("external alias should be valid");
    let mut imported = saved_workbench(
        alias.clone(),
        project_id.clone(),
        vec![RunId::from_string("missing")],
        "loss",
    );
    imported.views[0].name = "New Source Import".to_owned();
    imported
        .save(&external)
        .expect("external workbench should save");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    let configuration = window
        .read_with(&cx, |viewer, _| {
            viewer
                .source_configuration
                .clone()
                .expect("Viewer configuration should load")
        })
        .expect("viewer should remain open");
    let preparation = prepare_workbench_import(external, configuration)
        .expect("external workbench should preflight");
    assert_eq!(preparation.unresolved.len(), 1);
    let pending = finish_workbench_import(
        preparation,
        vec![(
            alias.clone(),
            SourcePreflight::load(source.path()).expect("new Source should preflight"),
        )],
    )
    .expect("new Source mapping should finish");
    assert!(!config_path.exists());

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.session.update(cx, |session, _| {
                session.workbench_path = Some(workbench_path.clone());
            });
            viewer.source_management.update(cx, |management, cx| {
                management.begin_workbench(pending.plan.clone(), cx);
            });
            viewer.pending_workbench_import = Some(pending);
        })
        .expect("viewer should remain open");
    cx.refresh().expect("confirmation should render");
    let confirm = cx
        .debug_bounds("confirm-workbench-import")
        .expect("import confirmation should render");
    cx.simulate_click(confirm.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().name == "New Source Import"
            && viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .find(|source| source.source_id.alias() == &alias)
                .is_some_and(|source| {
                    source.source_id.alias() == &alias
                        && source.project_allowlist == [project_id.clone()]
                })
    });

    let saved = std::fs::read_to_string(config_path).expect("new Source config should be saved");
    let saved = saved
        .parse::<toml_edit::DocumentMut>()
        .expect("new Source config should remain valid TOML");
    assert_eq!(
        saved["sources"]["portable"]["projects"]
            .as_array()
            .and_then(|projects| projects.get(0))
            .and_then(toml_edit::Value::as_str),
        Some(project_id.as_str())
    );
}

#[gpui::test]
fn ambiguous_workbench_alias_uses_the_selected_existing_source(cx: &mut TestAppContext) {
    let (first, one, two) = source_with_two_projects();
    let (second, _, _) = source_with_two_projects();
    let scope = tempfile::tempdir().expect("test scope should be created");
    let external = scope.path().join("external.toml");
    let external_alias = SourceAlias::new("portable").expect("external alias should be valid");
    saved_workbench(
        external_alias.clone(),
        two.clone(),
        vec![RunId::from_string("missing")],
        "loss",
    )
    .save(&external)
    .expect("external workbench should save");
    let (window, cx) = open_viewer(cx, Some(scope.path().to_owned()));
    let mut configuration = window
        .read_with(&cx, |viewer, _| {
            viewer
                .source_configuration
                .clone()
                .expect("Viewer configuration should load")
        })
        .expect("viewer should remain open");
    let first_alias = SourceAlias::new("first").expect("first alias should be valid");
    let second_alias = SourceAlias::new("second").expect("second alias should be valid");
    configuration
        .set_source(&first_alias, first.path(), std::slice::from_ref(&one))
        .expect("first candidate should configure");
    configuration
        .set_source(&second_alias, second.path(), std::slice::from_ref(&one))
        .expect("second candidate should configure");
    let preparation = prepare_workbench_import(external, configuration)
        .expect("ambiguous workbench should prepare");
    assert_eq!(preparation.unresolved[0].0, external_alias);

    let pending = finish_workbench_import(
        preparation,
        vec![(
            external_alias.clone(),
            SourcePreflight::load(second.path()).expect("selected Source should preflight"),
        )],
    )
    .expect("explicit existing Source mapping should finish");

    assert_eq!(
        pending.plan.alias_rewrites,
        [(external_alias, second_alias.clone())]
    );
    assert_eq!(
        pending.plan.allowlist_additions,
        [(second_alias.clone(), vec![two])]
    );
    assert_eq!(
        pending
            .configuration
            .sources
            .iter()
            .find(|source| source.configured.alias == second_alias)
            .expect("selected Source should remain configured")
            .configured
            .projects,
        [one, ProjectId::from_string("two")]
    );
}

#[gpui::test]
fn invalid_or_unwritable_workbench_import_keeps_live_state(cx: &mut TestAppContext) {
    let (source, one, _) = source_with_two_projects();
    let scope = tempfile::tempdir().expect("test scope should be created");
    let invalid = scope.path().join("invalid.toml");
    std::fs::write(&invalid, "schema_version = 99\n")
        .expect("invalid external workbench should be written");
    let (window, mut cx) = open_viewer(cx, Some(scope.path().to_owned()));
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.preflight_workbench_path(invalid, window, cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session.read(cx).transient_error.is_some()
    });
    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer
                .source_management
                .read(cx)
                .is_open())
            .expect("viewer should remain open")
    );

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.save_confirmed_source(
                ConfirmedSource {
                    manage: false,
                    alias: SourceAlias::new("research").expect("alias should be valid"),
                    root_path: source.path().to_owned(),
                    projects: vec![one.clone()],
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .any(|source| source.source_id.alias().as_str() == "research")
    });
    let external = scope.path().join("external.toml");
    saved_workbench(
        SourceAlias::new("research").expect("alias should be valid"),
        one,
        vec![RunId::from_string("missing")],
        "loss",
    )
    .save(&external)
    .expect("external workbench should save");
    let destination = scope.path().join("unwritable-workbench");
    std::fs::create_dir(&destination).expect("destination directory should be created");
    window
        .update(&mut cx, |viewer, window, cx| {
            viewer.session.update(cx, |session, _| {
                session.workbench_path = Some(destination);
            });
            viewer.preflight_workbench_path(external, window, cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.source_management.read(cx).is_open()
    });
    cx.refresh().expect("confirmation should render");
    let confirm = cx
        .debug_bounds("confirm-workbench-import")
        .expect("import confirmation should render");
    cx.simulate_click(confirm.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session
            .read(cx)
            .transient_error
            .as_ref()
            .is_some_and(|error| error.contains("directory"))
    });
    assert_ne!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .active()
                .name
                .clone())
            .expect("viewer should remain open"),
        "Restored"
    );
}
