//! Bounded text paging so inline layout cost does not grow with payload size.

use std::ops::Range;

use sift_core::body::MAX_INLINE_TEXT_BYTES;

// Allow typing into a full page without resetting its cursor on each key.
const MAX_EDIT_TEXT_BYTES: usize = MAX_INLINE_TEXT_BYTES * 2;

pub(crate) fn page_count(bytes: usize) -> usize {
    bytes.div_ceil(MAX_INLINE_TEXT_BYTES).max(1)
}

pub(crate) fn page_range(text: &str, page: usize) -> Range<usize> {
    let page = page.min(page_count(text.len()) - 1);
    let mut start = page * MAX_INLINE_TEXT_BYTES;
    let mut end = text.len().min(start + MAX_INLINE_TEXT_BYTES);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    start..end
}

pub(crate) fn page_controls(ui: &mut egui::Ui, bytes: usize, page: &mut usize) -> bool {
    let before = *page;
    let count = page_count(bytes);
    *page = (*page).min(count - 1);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(*page > 0, egui::Button::new("Previous"))
            .clicked()
        {
            *page -= 1;
        }
        ui.label(format!("Page {} of {count}", *page + 1));
        if ui
            .add_enabled(*page + 1 < count, egui::Button::new("Next"))
            .clicked()
        {
            *page += 1;
        }
        let mut number = *page + 1;
        if ui
            .add(
                egui::DragValue::new(&mut number)
                    .range(1..=count)
                    .prefix("Go to "),
            )
            .changed()
        {
            *page = number - 1;
        }
    });
    before != *page
}

#[derive(Debug)]
pub(crate) struct PagedEditor {
    page: usize,
    range: Range<usize>,
    text: String,
    source_ptr: usize,
    source_len: usize,
}

impl PagedEditor {
    fn new(body: &str, page: usize) -> Self {
        let page = page.min(page_count(body.len()) - 1);
        let range = page_range(body, page);
        Self {
            page,
            text: body[range.clone()].to_owned(),
            range,
            source_ptr: body.as_ptr() as usize,
            source_len: body.len(),
        }
    }

    fn current(&self, body: &str) -> bool {
        self.source_ptr == body.as_ptr() as usize
            && self.source_len == body.len()
            && self.text.len() <= MAX_EDIT_TEXT_BYTES
    }

    #[cfg(test)]
    pub(crate) fn visible_bytes(&self) -> usize {
        self.text.len()
    }

    fn apply(&mut self, body: &mut String) {
        body.replace_range(self.range.clone(), &self.text);
        self.range.end = self.range.start + self.text.len();
        self.source_ptr = body.as_ptr() as usize;
        self.source_len = body.len();
    }
}

// TextEdit processes paste before layout. Cap that layout too, then let the
// next frame page the complete insertion; never cap the editable buffer.
fn bounded_layout(
    ui: &egui::Ui,
    buffer: &dyn egui::TextBuffer,
    wrap_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let text = sift_core::body::bounded_text(buffer.as_str(), MAX_EDIT_TEXT_BYTES).to_owned();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    ui.fonts_mut(|fonts| fonts.layout(text, font, ui.visuals().text_color(), wrap_width))
}

pub(crate) fn editor(
    ui: &mut egui::Ui,
    body: &mut String,
    state: &mut Option<PagedEditor>,
) -> egui::Response {
    let mut layouter = bounded_layout;
    if body.len() <= MAX_INLINE_TEXT_BYTES {
        *state = None;
        return ui.add(
            egui::TextEdit::multiline(body)
                .code_editor()
                .layouter(&mut layouter)
                .hint_text("message body")
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
    }
    let page = state.as_ref().map_or(0, |editor| editor.page);
    if state.as_ref().is_none_or(|editor| !editor.current(body)) {
        *state = Some(PagedEditor::new(body, page));
    }
    let editor = state.as_mut().expect("paged editor initialized");
    ui.label(format!(
        "Large payload: {} bytes. Edit one page at a time; the complete body will be sent.",
        body.len()
    ));
    if page_controls(ui, body.len(), &mut editor.page) {
        *editor = PagedEditor::new(body, editor.page);
    }
    ui.label(
        egui::RichText::new(format!(
            "Bytes {}–{} of {}",
            editor.range.start + 1,
            editor.range.end,
            body.len()
        ))
        .weak(),
    );
    let response = ui.add(
        egui::TextEdit::multiline(&mut editor.text)
            .id_salt(("payload-page", editor.page))
            .code_editor()
            .layouter(&mut layouter)
            .desired_rows(8)
            .desired_width(f32::INFINITY),
    );
    if response.changed() {
        editor.apply(body);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_reassemble_unicode_body_without_gaps_or_split_characters() {
        let body = "🦀é中".repeat(20_000);
        let reconstructed: String = (0..page_count(body.len()))
            .map(|page| &body[page_range(&body, page)])
            .collect();
        assert_eq!(reconstructed, body);
        for page in 0..page_count(body.len()) {
            assert!(page_range(&body, page).len() <= MAX_INLINE_TEXT_BYTES + 3);
        }
    }

    #[test]
    fn editing_middle_page_preserves_every_byte_outside_the_edit() {
        let mut body = "a".repeat(1_400_000);
        let mut editor = PagedEditor::new(&body, 17);
        let original = editor.range.clone();
        editor.text.replace_range(100..105, "changed 🦀");
        editor.apply(&mut body);
        assert_eq!(
            &body[..original.start + 100],
            "a".repeat(original.start + 100)
        );
        assert!(body[original.start + 100..].starts_with("changed 🦀"));
        assert!(body[editor.range.end..].chars().all(|c| c == 'a'));
        assert_eq!(body.len(), 1_400_000 - 5 + "changed 🦀".len());
        assert!(editor.current(&body));
    }

    #[test]
    fn large_paste_is_saved_in_full_while_that_frames_layout_stays_bounded() {
        let mut body = "a".repeat(1_400_000);
        let paste = "🦀".repeat(350_000);
        let mut state = None;
        let ctx = egui::Context::default();
        for frame in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 900.0),
                )),
                events: if frame == 1 {
                    vec![egui::Event::Paste(paste.clone())]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let response = editor(ui, &mut body, &mut state);
                if frame == 0 {
                    response.request_focus();
                }
            });
            for shape in output.shapes {
                if let egui::epaint::Shape::Text(text) = shape.shape {
                    assert!(
                        text.galley.text().len() <= MAX_EDIT_TEXT_BYTES,
                        "paste layout must remain bounded"
                    );
                }
            }
        }
        assert_eq!(body.len(), 1_400_000 + paste.len());
        assert!(body.contains(&paste));
        assert!(
            state.as_ref().expect("large body is paged").visible_bytes()
                <= MAX_INLINE_TEXT_BYTES + 3
        );
    }
}
