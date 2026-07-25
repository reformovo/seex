use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

const REFRESH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="none"><path stroke="#000" stroke-linecap="round" stroke-width="1.2" d="M4.667 14V8m0 0H2m2.667 0h2.666"/><path stroke="#000" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.2" d="M14 12.667h-3.333V9.333"/><path fill="#000" d="M4.03 3.5a.667.667 0 0 0 .883 1l-.882-1ZM8 3.333A4.667 4.667 0 0 1 12.666 8H14a6 6 0 0 0-6-6v1.333ZM12.666 8a4.656 4.656 0 0 1-1.75 3.643l.834 1.04A5.99 5.99 0 0 0 14 8h-1.334ZM5.667 3.957A4.642 4.642 0 0 1 8 3.333V2a5.976 5.976 0 0 0-3 .803l.667 1.154Zm-.754.543c.232-.205.485-.387.754-.543l-.668-1.154c-.346.2-.67.434-.968.697l.882 1Z"/></svg>"##;
const PLUS: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M3.33325 8H12.6666" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/><path d="M8 3.33333V12.6667" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CLOSE: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M4.70581 4.5L11.294 11.5M11.294 4.5L4.70581 11.5" stroke="black" stroke-width="1.2" stroke-linecap="round"/></svg>"##;
const ELLIPSIS: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="3" cy="8" r="1" fill="black" stroke="black" stroke-width="0.5"/><circle cx="8" cy="8" r="1" fill="black" stroke="black" stroke-width="0.5"/><circle cx="13" cy="8" r="1" fill="black" stroke="black" stroke-width="0.5"/></svg>"##;
const ARCHIVE: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M2.5 4.5h11v9h-11zM2 2.5h12v2H2zM6 7h4" fill="none" stroke="black" stroke-width="1.2" stroke-linejoin="round"/></svg>"##;
const BASELINE: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M8 13V3m0 0L4.5 6.5M8 3l3.5 3.5" fill="none" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CIRCLE_PLAY: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><circle cx="8" cy="8" r="5.5" fill="none" stroke="black" stroke-width="1.2"/><path d="m6.8 5.5 3.5 2.5-3.5 2.5z" fill="black"/></svg>"##;
const EYE: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M1.8 8s2.2-3.5 6.2-3.5S14.2 8 14.2 8 12 11.5 8 11.5 1.8 8 1.8 8Z" fill="none" stroke="black" stroke-width="1.2"/><circle cx="8" cy="8" r="1.7" fill="none" stroke="black" stroke-width="1.2"/></svg>"##;
const EYE_CLOSED: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M2 8.5s2.2 2.5 6 2.5 6-2.5 6-2.5M4 11l-1 2m4-1.5L6.7 14m5.3-3-1 2" fill="none" stroke="black" stroke-width="1.2" stroke-linecap="round"/></svg>"##;
const EYE_OFF: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M2 2l12 12M6.5 4.7A7 7 0 0 1 8 4.5c4 0 6.2 3.5 6.2 3.5a9 9 0 0 1-2 2.2M9.5 11.3A7 7 0 0 1 8 11.5C4 11.5 1.8 8 1.8 8a9 9 0 0 1 2-2.2" fill="none" stroke="black" stroke-width="1.2" stroke-linecap="round"/></svg>"##;
const FOLDER: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M1.8 4.2h5l1.2 1.5h6.2v7.1H1.8z" fill="none" stroke="black" stroke-width="1.2" stroke-linejoin="round"/></svg>"##;
const FOLDER_OPEN: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M2 5V3.5h4.5L8 5h6v2M2 6.8h12.5l-1.8 6H3.2z" fill="none" stroke="black" stroke-width="1.2" stroke-linejoin="round"/></svg>"##;
const PIN: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="m5 2 6 6-2 1.2-.7 3.3-4.8-4.8L6.8 7zM3.5 12.5l2.2-2.2" fill="none" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const ARROW_UP: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><path d="M8 13V3m0 0L4.5 6.5M8 3l3.5 3.5" fill="none" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CLOCK: &[u8] = br##"<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg"><circle cx="8" cy="8" r="5.5" fill="none" stroke="black" stroke-width="1.2"/><path d="M8 4.8V8l2.3 1.4" fill="none" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;

const ASSETS: [(&str, &[u8]); 15] = [
    ("icons/refresh.svg", REFRESH),
    ("icons/plus.svg", PLUS),
    ("icons/close.svg", CLOSE),
    ("icons/ellipsis.svg", ELLIPSIS),
    ("icons/archive.svg", ARCHIVE),
    ("icons/baseline.svg", BASELINE),
    ("icons/circle-play.svg", CIRCLE_PLAY),
    ("icons/eye.svg", EYE),
    ("icons/eye-closed.svg", EYE_CLOSED),
    ("icons/eye-off.svg", EYE_OFF),
    ("icons/folder.svg", FOLDER),
    ("icons/folder-open.svg", FOLDER_OPEN),
    ("icons/pin.svg", PIN),
    ("icons/arrow-up.svg", ARROW_UP),
    ("icons/clock.svg", CLOCK),
];

pub struct ViewerAssets;

impl AssetSource for ViewerAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS
            .iter()
            .find_map(|(asset_path, bytes)| (*asset_path == path).then_some(Cow::Borrowed(*bytes))))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        Ok(ASSETS
            .iter()
            .filter_map(|(asset_path, _)| {
                asset_path
                    .strip_prefix(prefix)
                    .and_then(|suffix| suffix.strip_prefix('/'))
                    .filter(|suffix| !suffix.contains('/'))
                    .map(SharedString::from)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_paths_are_loadable_and_listed() -> gpui::Result<()> {
        let assets = ViewerAssets;

        for (path, _) in ASSETS {
            assert!(
                assets.load(path)?.is_some(),
                "missing embedded asset {path}"
            );
        }
        assert_eq!(assets.list("icons")?.len(), ASSETS.len());
        assert!(assets.load("icons/missing.svg")?.is_none());
        Ok(())
    }
}
