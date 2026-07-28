use std::time::Duration;

use gpui::{Context, KeyDownEvent};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EditOutcome {
    pub changed: bool,
    pub handled: bool,
}

/// Shared single-line editing state for filters and in-place renames.
#[derive(Default)]
pub(crate) struct TextInput {
    text: String,
    cursor: usize,
    select_all: bool,
    cursor_visible: bool,
    blink_epoch: u64,
}

impl TextInput {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_select_all(&self) -> bool {
        self.select_all
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.select_all = false;
    }

    pub fn clear(&mut self) -> bool {
        if self.text.is_empty() {
            self.cursor = 0;
            self.select_all = false;
            return false;
        }
        self.text.clear();
        self.cursor = 0;
        self.select_all = false;
        true
    }

    pub fn select_all(&mut self) {
        self.cursor = self.text.len();
        self.select_all = true;
    }

    pub fn move_to_end(&mut self) {
        self.cursor = self.text.len();
        self.select_all = false;
    }

    pub fn edit(&mut self, event: &KeyDownEvent) -> EditOutcome {
        let changed = match event.keystroke.key.as_str() {
            "left" => {
                self.cursor = if self.select_all {
                    0
                } else {
                    previous_text_cursor(&self.text, self.cursor)
                };
                self.select_all = false;
                false
            }
            "right" => {
                self.cursor = if self.select_all {
                    self.text.len()
                } else {
                    next_text_cursor(&self.text, self.cursor)
                };
                self.select_all = false;
                false
            }
            "backspace" => {
                if self.select_all {
                    self.text.clear();
                    self.cursor = 0;
                    self.select_all = false;
                    true
                } else {
                    let previous = previous_text_cursor(&self.text, self.cursor);
                    let changed = previous != self.cursor;
                    self.text.replace_range(previous..self.cursor, "");
                    self.cursor = previous;
                    changed
                }
            }
            _ if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control => {
                let Some(text) = event.keystroke.key_char.as_deref() else {
                    return EditOutcome::default();
                };
                if text.chars().any(char::is_control) {
                    return EditOutcome::default();
                }
                if self.select_all {
                    self.text.clear();
                    self.cursor = 0;
                }
                self.text.insert_str(self.cursor, text);
                self.cursor += text.len();
                self.select_all = false;
                true
            }
            _ => return EditOutcome::default(),
        };
        EditOutcome {
            changed,
            handled: true,
        }
    }

    pub fn start_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch = self.blink_epoch.saturating_add(1);
        self.cursor_visible = true;
        self.schedule_blink(cx);
        cx.notify();
    }

    pub fn stop_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch = self.blink_epoch.saturating_add(1);
        self.cursor_visible = false;
        cx.notify();
    }

    fn schedule_blink(&self, cx: &mut Context<Self>) {
        let epoch = self.blink_epoch;
        let timer = cx.background_executor().timer(Duration::from_millis(500));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.blink_epoch != epoch {
                    return;
                }
                this.cursor_visible = !this.cursor_visible;
                this.schedule_blink(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

fn previous_text_cursor(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_text_cursor(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |character| cursor + character.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::{next_text_cursor, previous_text_cursor};

    #[test]
    fn cursor_navigation_respects_utf8_boundaries() {
        let text = "a界b";
        assert_eq!(next_text_cursor(text, 1), 4);
        assert_eq!(previous_text_cursor(text, 4), 1);
    }
}
