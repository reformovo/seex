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
        ImportSource,
        Refresh,
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
    Application::new()
        .with_assets(ViewerAssets)
        .run(move |cx: &mut App| {
            cx.bind_keys([
                KeyBinding::new("cmd-o", ImportSource, None),
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
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(menus());
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
        });
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
                MenuItem::action("Import Source…", ImportSource),
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
                MenuItem::action("Absolute Time", UseElapsed),
            ],
        },
    ]
}
