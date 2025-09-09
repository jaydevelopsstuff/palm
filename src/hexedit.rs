use eframe::egui::text::CCursorRange;
use eframe::egui::text_edit::TextEditState;
use eframe::egui::{self, TextEdit};
use eframe::egui::{Key, Widget};
use eframe::epaint::text::cursor;

use crate::util::hex_encode_formatted;

pub struct HexEditor<'a> {
    buffer: &'a mut Vec<u8>,
    view: String,
}

impl<'a> HexEditor<'a> {
    pub fn new(buffer: &'a mut Vec<u8>) -> Self {
        Self {
            view: hex::encode_upper(&buffer)
                .chars()
                .enumerate()
                .flat_map(|(i, c)| {
                    if i != 0 && i % 2 == 0 {
                        Some(' ')
                    } else {
                        None
                    }
                    .into_iter()
                    .chain(std::iter::once(c))
                })
                .collect::<String>(),
            buffer,
        }
    }

    fn handle_event(&self, event: &egui::Event, ctx: &egui::Context) -> (EventHandleResult, bool) {
        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } => match key {
                Key::ArrowLeft if *pressed => {
                    (EventHandleResult::CursorLeft(modifiers.shift), true)
                }
                Key::ArrowRight if *pressed => {
                    (EventHandleResult::CursorRight(modifiers.shift), true)
                }
                Key::Backspace if *pressed => (EventHandleResult::Delete, true),
                _ => (EventHandleResult::NoAction, false),
            },
            egui::Event::Cut => (EventHandleResult::Cut, true),
            egui::Event::Text(text) => (EventHandleResult::Text(text.clone()), true),
            egui::Event::Paste(text) => (EventHandleResult::Paste(text.clone()), true),
            _ => (EventHandleResult::NoAction, false),
        }
    }

    fn process_event_result(
        &mut self,
        result: EventHandleResult,
        focused: bool,
        partial_nibble: &mut PartialNibble,
        state: &mut TextEditState,
        ctx: &egui::Context,
    ) {
        if !focused {
            return;
        }
        match result {
            EventHandleResult::Delete => {
                if let Some(cursor_range) = state.cursor.char_range() {
                    let p_buf_index = view_index_to_buffer_index(cursor_range.primary.index);
                    let s_buf_index = view_index_to_buffer_index(cursor_range.secondary.index);
                    if s_buf_index == p_buf_index && p_buf_index != 0 {
                        self.buffer.remove(p_buf_index - 1);
                    } else {
                        self.buffer.drain(
                            usize::min(p_buf_index, s_buf_index)
                                ..usize::max(p_buf_index, s_buf_index),
                        );
                    }
                }
            }
            EventHandleResult::Paste(text) => {
                if let Some(mut cursor_range) = state.cursor.char_range() {
                    let p_buf_index = view_index_to_buffer_index(cursor_range.primary.index);
                    let s_buf_index = view_index_to_buffer_index(cursor_range.secondary.index);

                    let mut cleaned_text: String =
                        text.chars().filter(|c| c.is_digit(16)).collect();

                    if cleaned_text.len() % 2 != 0 {
                        cleaned_text.pop();
                    }

                    if let Ok(data) = hex::decode(cleaned_text) {
                        let data_len = data.len();
                        self.buffer.splice(
                            usize::min(p_buf_index, s_buf_index)
                                ..usize::max(p_buf_index, s_buf_index),
                            data,
                        );

                        // Move cursor to right after what we just inserted (and reset selection)
                        cursor_range.primary.index =
                            cursor_range.primary.index.min(cursor_range.secondary.index)
                                + data_len * 2;
                        cursor_range.secondary.index = cursor_range.primary.index;
                        state.cursor.set_char_range(Some(cursor_range));
                    }
                }
            }
            EventHandleResult::Cut => {
                if let Some(mut cursor_range) = state.cursor.char_range() {
                    let pc_i = cursor_range.primary.index;
                    let sc_i = cursor_range.secondary.index;
                    let p_buf_index = view_index_to_buffer_index(pc_i);
                    let s_buf_index = view_index_to_buffer_index(sc_i);

                    ctx.copy_text(self.view[pc_i.min(sc_i)..pc_i.max(sc_i)].trim().into());
                    self.buffer
                        .drain(p_buf_index.min(s_buf_index)..p_buf_index.max(s_buf_index));

                    // Move cursor to beginnning of what we just cut
                    cursor_range.primary.index = pc_i.min(sc_i);
                    cursor_range.secondary.index = cursor_range.primary.index;
                    state.cursor.set_char_range(Some(cursor_range));
                }
            }
            EventHandleResult::Text(text) => {
                if let Some(mut cursor_range) = state.cursor.char_range() {
                    let p_buf_index = view_index_to_buffer_index(cursor_range.primary.index);
                    let s_buf_index = view_index_to_buffer_index(cursor_range.secondary.index);
                    if let Some(partial_nibble_inner) = partial_nibble.0 {
                        if let Ok(byte) = hex::decode(format!("{partial_nibble_inner}{text}")) {
                            self.buffer.splice(
                                usize::min(p_buf_index, s_buf_index)
                                    ..usize::max(p_buf_index, s_buf_index),
                                byte,
                            );
                            partial_nibble.0 = None;
                            // Move cursor to right after what we just inserted (and reset selection)
                            cursor_range.primary.index = usize::min(
                                cursor_range.primary.index,
                                cursor_range.secondary.index,
                            ) + 2;
                            cursor_range.secondary.index = cursor_range.primary.index;
                            state.cursor.set_char_range(Some(cursor_range));
                        }
                    } else {
                        partial_nibble.0 = Some(text.chars().next().unwrap());
                    }
                }
            }
            _ => {}
        }
    }

    fn sync_view(&mut self) {
        self.view = hex_encode_formatted(&self.buffer);
    }
}

impl Widget for HexEditor<'_> {
    fn ui(mut self, ui: &mut egui::Ui) -> egui::Response {
        let mut event_results = vec![];

        ui.input_mut(|i| {
            i.events.retain(|event| {
                let (result, should_consume) = self.handle_event(&event, ui.ctx());

                event_results.push(result);

                !should_consume
            });
        });
        let output = TextEdit::multiline(&mut self.view).show(ui);

        let mut state = output.state.clone();
        let mut partial_nibble = PartialNibble(None);

        ui.data(|r| {
            partial_nibble = r.get_temp(output.response.id).unwrap_or_default();
        });

        if let Some(mut cursor_range) = state.cursor.char_range() {
            let primary = &mut cursor_range.primary.index;
            let secondary = &mut cursor_range.secondary.index;
            *primary = if *primary != 0 {
                *primary - *primary % 3 + 2
            } else {
                0
            };
            *secondary = if *secondary != 0 {
                *secondary - *secondary % 3 + 2
            } else {
                0
            };

            for result in &event_results {
                match *result {
                    EventHandleResult::CursorLeft(shift_pressed) => {
                        if *primary >= 3 {
                            *primary -= 3;
                        } else {
                            *primary = 0;
                        }
                        if !shift_pressed {
                            *secondary = *primary;
                        }
                    }
                    EventHandleResult::CursorRight(shift_pressed) => {
                        if *primary == 0 {
                            *primary = 2;
                        } else if *primary + 3 <= self.view.len() {
                            *primary += 3;
                        }
                        if !shift_pressed {
                            *secondary = *primary;
                        }
                    }
                    _ => {}
                }
            }
            state.cursor.set_char_range(Some(cursor_range));
        }

        for result in event_results {
            self.process_event_result(
                result,
                output.response.has_focus(),
                &mut partial_nibble,
                &mut state,
                ui.ctx(),
            );
        }
        self.sync_view();
        ui.data_mut(|w| w.insert_temp(output.response.id, partial_nibble));
        state.store(ui.ctx(), output.response.id);

        output.response
    }
}

#[derive(Clone, Default)]
struct PartialNibble(Option<char>);

fn view_index_to_buffer_index(view_cursor: usize) -> usize {
    (view_cursor + 2) / 3
}

#[derive(PartialEq, Eq)]
enum EventHandleResult {
    Text(String),
    Paste(String),
    Delete,
    Cut,
    CursorLeft(bool),
    CursorRight(bool),
    NoAction,
}
