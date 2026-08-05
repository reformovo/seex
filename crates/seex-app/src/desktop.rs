//! macOS GPUI desktop entry point.

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, SystemMenuType, WindowBounds,
    WindowOptions, actions, px, size,
};

mod app;

use app::{ViewerApp, ViewerAssets};

actions!(
    seex_app,
    [
        Refresh,
        OpenSources,
        ReloadSources,
        ImportWorkbench,
        ExportWorkbench,
        ResetView,
        ToggleProjectSidebar,
        ToggleMetricSidebar,
        ToggleBottomInspector,
        ShowMetricInspector,
        ClearLockedCursor,
        ZoomIn,
        ZoomOut,
        UseStep,
        UseElapsed,
        ActivateSelection,
        Quit
    ]
);

pub(super) const SELECTABLE_CONTEXT: &str = "ViewerSelectable";

pub fn run(project_path: Option<PathBuf>) {
    let app = Application::new().with_assets(ViewerAssets);
    let reopen_project_path = project_path.clone();
    app.on_reopen(move |cx| {
        if cx.windows().is_empty() {
            open_viewer_window(reopen_project_path.clone(), cx);
        } else {
            cx.activate(true);
        }
    });
    app.run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-r", Refresh, None),
            KeyBinding::new("cmd-shift-b", ToggleProjectSidebar, None),
            KeyBinding::new("cmd-shift-m", ToggleMetricSidebar, None),
            KeyBinding::new("cmd-j", ToggleBottomInspector, None),
            KeyBinding::new("cmd-0", ResetView, None),
            KeyBinding::new("cmd-=", ZoomIn, None),
            KeyBinding::new("cmd-shift-=", ZoomIn, None),
            KeyBinding::new("cmd-+", ZoomIn, None),
            KeyBinding::new("cmd--", ZoomOut, None),
            KeyBinding::new("escape", ClearLockedCursor, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("enter", ActivateSelection, Some(SELECTABLE_CONTEXT)),
            KeyBinding::new("space", ActivateSelection, Some(SELECTABLE_CONTEXT)),
        ]);
        cx.set_menus(menus());
        open_viewer_window(project_path, cx);
    });
}

fn open_viewer_window(project_path: Option<PathBuf>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1_200.), px(800.)), cx);
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..WindowOptions::default()
        },
        move |window, cx| cx.new(|cx| ViewerApp::new(project_path, window, cx)),
    );
    if let Err(error) = result {
        eprintln!("failed to open seex-app window: {error}");
        cx.quit();
    } else {
        cx.activate(true);
    }
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Seex".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Seex", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Sources…", OpenSources),
                MenuItem::action("Reload Sources", ReloadSources),
                MenuItem::action("Import Workbench…", ImportWorkbench),
                MenuItem::action("Export Workbench…", ExportWorkbench),
                MenuItem::separator(),
                MenuItem::action("Refresh", Refresh),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Reset View", ResetView),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::separator(),
                MenuItem::action("Toggle Project Sidebar", ToggleProjectSidebar),
                MenuItem::action("Toggle Metric Sidebar", ToggleMetricSidebar),
                MenuItem::action("Toggle Bottom Inspector", ToggleBottomInspector),
                MenuItem::action("Show Metric Inspector", ShowMetricInspector),
                MenuItem::separator(),
                MenuItem::action("Step", UseStep),
                MenuItem::action("Elapsed Time", UseElapsed),
            ],
        },
    ]
}
