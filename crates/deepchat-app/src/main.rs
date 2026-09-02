use std::ops::Range;

use gpui::{
    App, Application, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, ScrollHandle, ShapedLine, SharedString, Style, TextRun, TitlebarOptions,
    UTF16Selection, UnderlineStyle, Window, WindowBounds, WindowOptions, actions, div, fill, hsla,
    point, prelude::*, px, rgb, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    composer,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Submit,
        InsertNewline,
        Quit,
    ]
);

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditSnapshot {
    text: String,
    selection: Range<usize>,
    selection_reversed: bool,
}

#[derive(Debug)]
struct ComposerState {
    text: String,
    selection: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    composition_snapshot: Option<EditSnapshot>,
    undo: Vec<EditSnapshot>,
    redo: Vec<EditSnapshot>,
    submit_attempts: usize,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            text: String::new(),
            selection: 0..0,
            selection_reversed: false,
            marked_range: None,
            composition_snapshot: None,
            undo: Vec::new(),
            redo: Vec::new(),
            submit_attempts: 0,
        }
    }
}

impl ComposerState {
    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.snapshot());
        self.redo.clear();
    }

    fn restore(&mut self, snapshot: EditSnapshot) {
        self.text = snapshot.text;
        self.selection = snapshot.selection;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.composition_snapshot = None;
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.undo.pop() {
            let current = self.snapshot();
            self.restore(snapshot);
            self.redo.push(current);
        }
    }

    fn redo(&mut self) {
        if let Some(snapshot) = self.redo.pop() {
            let current = self.snapshot();
            self.restore(snapshot);
            self.undo.push(current);
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.text.chars() {
            if utf16 >= offset {
                break;
            }
            let next = utf16 + ch.len_utf16();
            if offset < next {
                break;
            }
            utf16 = next;
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn range_from_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn range_to_utf16(&self, range: Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selection.clone())
    }

    fn replace(&mut self, range_utf16: Option<Range<usize>>, new_text: &str) {
        let range = self.replacement_range(range_utf16);
        if let Some(snapshot) = self.composition_snapshot.take() {
            self.undo.push(snapshot);
            self.redo.clear();
        } else {
            self.push_undo();
        }
        self.text.replace_range(range.clone(), new_text);
        let cursor = range.start + new_text.len();
        self.selection = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
    }

    fn replace_and_mark(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        selected_in_mark_utf16: Option<Range<usize>>,
    ) {
        let range = self.replacement_range(range_utf16);
        if self.composition_snapshot.is_none() {
            self.composition_snapshot = Some(self.snapshot());
            self.redo.clear();
        }
        self.text.replace_range(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selection = if let Some(selected) = selected_in_mark_utf16 {
            let marked_text = &self.text[range.start..range.start + new_text.len()];
            let relative = utf16_range_in(marked_text, selected);
            range.start + relative.start..range.start + relative.end
        } else {
            let cursor = range.start + new_text.len();
            cursor..cursor
        };
        self.selection_reversed = false;
    }

    fn unmark(&mut self) {
        if let Some(snapshot) = self.composition_snapshot.take() {
            self.undo.push(snapshot);
            self.redo.clear();
        }
        self.marked_range = None;
    }

    fn submit(&mut self) -> bool {
        if self.composition_snapshot.is_some() {
            return false;
        }
        self.submit_attempts += 1;
        false
    }

    fn cursor(&self) -> usize {
        if self.selection_reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.text.len())
    }

    fn move_to(&mut self, offset: usize) {
        self.selection = offset..offset;
        self.selection_reversed = false;
    }

    fn select_to(&mut self, offset: usize) {
        if self.selection_reversed {
            self.selection.start = offset;
        } else {
            self.selection.end = offset;
        }
        if self.selection.end < self.selection.start {
            self.selection_reversed = !self.selection_reversed;
            self.selection = self.selection.end..self.selection.start;
        }
    }
}

fn editable_index_for_hit(text: &str, candidate: usize) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut index = candidate.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn utf16_range_in(text: &str, range: Range<usize>) -> Range<usize> {
    fn convert(text: &str, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in text.chars() {
            if utf16 >= offset {
                break;
            }
            let next = utf16 + ch.len_utf16();
            if offset < next {
                break;
            }
            utf16 = next;
            utf8 += ch.len_utf8();
        }
        utf8
    }
    convert(text, range.start)..convert(text, range.end)
}

struct Composer {
    focus: FocusHandle,
    state: ComposerState,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl Composer {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            state: ComposerState::default(),
            last_layout: None,
            last_bounds: None,
            selecting: false,
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let next = if self.state.selection.is_empty() {
            self.state.previous_boundary(self.state.cursor())
        } else {
            self.state.selection.start
        };
        self.state.move_to(next);
        self.changed(cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let next = if self.state.selection.is_empty() {
            self.state.next_boundary(self.state.cursor())
        } else {
            self.state.selection.end
        };
        self.state.move_to(next);
        self.changed(cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.state.previous_boundary(self.state.cursor());
        self.state.select_to(next);
        self.changed(cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.state.next_boundary(self.state.cursor());
        self.state.select_to(next);
        self.changed(cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.state.selection = 0..self.state.text.len();
        self.state.selection_reversed = false;
        self.changed(cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.state.move_to(0);
        self.changed(cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.state.move_to(self.state.text.len());
        self.changed(cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.selection.is_empty() {
            let previous = self.state.previous_boundary(self.state.cursor());
            self.state.select_to(previous);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.selection.is_empty() {
            let next = self.state.next_boundary(self.state.cursor());
            self.state.select_to(next);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.state.text[self.state.selection.clone()].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
        if !self.state.selection.is_empty() {
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.state.undo();
        self.changed(cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.state.redo();
        self.changed(cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        self.state.submit();
        self.changed(cx);
    }

    fn insert_newline(&mut self, _: &InsertNewline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        if self.state.text.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (&self.last_bounds, &self.last_layout) else {
            return 0;
        };
        if position.x <= bounds.left() {
            0
        } else if position.x >= bounds.right() {
            self.state.text.len()
        } else {
            editable_index_for_hit(
                &self.state.text,
                line.closest_index_for_x(position.x - bounds.left()),
            )
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = true;
        let index = self.index_for_position(event.position);
        if event.modifiers.shift {
            self.state.select_to(index);
        } else {
            self.state.move_to(index);
        }
        self.changed(cx);
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let index = self.index_for_position(event.position);
            self.state.select_to(index);
            self.changed(cx);
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
}

impl Focusable for Composer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for Composer {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf8 = self.state.range_from_utf16(range);
        adjusted_range.replace(self.state.range_to_utf16(utf8.clone()));
        Some(self.state.text[utf8].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.state.range_to_utf16(self.state.selection.clone()),
            reversed: self.state.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.state
            .marked_range
            .clone()
            .map(|range| self.state.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.state.unmark();
        self.changed(cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.replace(range, text);
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.replace_and_mark(range, new_text, selected);
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let utf8 = self.state.range_from_utf16(range);
        Some(Bounds::from_corners(
            point(bounds.left() + layout.x_for_index(utf8.start), bounds.top()),
            point(
                bounds.left() + layout.x_for_index(utf8.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.state.offset_to_utf16(self.index_for_position(point)))
    }
}

struct ComposerElement {
    composer: Entity<Composer>,
}

struct ComposerPrepaint {
    editable_line: ShapedLine,
    placeholder_line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

fn composer_display_text(text: &str) -> String {
    text.chars()
        .map(|character| if character == '\n' { ' ' } else { character })
        .collect()
}

impl IntoElement for ComposerElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ComposerElement {
    type RequestLayoutState = ();
    type PrepaintState = ComposerPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = px(42.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let composer = self.composer.read(cx);
        let display: SharedString = composer_display_text(&composer.state.text).into();
        let text_color = rgb(0x202124).into();
        let base = TextRun {
            len: display.len(),
            font: window.text_style().font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = &composer.state.marked_range {
            vec![
                TextRun {
                    len: marked.start,
                    ..base.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(text_color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..base.clone()
                },
                TextRun {
                    len: display.len().saturating_sub(marked.end),
                    ..base
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![base]
        };
        let editable_line = window
            .text_system()
            .shape_line(display, px(15.), &runs, None);
        let placeholder_line = composer.state.text.is_empty().then(|| {
            let placeholder: SharedString = "Message DeepChat".into();
            let placeholder_run = TextRun {
                len: placeholder.len(),
                font: window.text_style().font(),
                color: hsla(0., 0., 0.4, 1.),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(placeholder, px(15.), &[placeholder_run], None)
        });
        let selection = (!composer.state.selection.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(
                        bounds.left() + editable_line.x_for_index(composer.state.selection.start),
                        bounds.top(),
                    ),
                    point(
                        bounds.left() + editable_line.x_for_index(composer.state.selection.end),
                        bounds.bottom(),
                    ),
                ),
                rgba(0x3b82f633),
            )
        });
        let cursor = composer.state.selection.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(
                        bounds.left() + editable_line.x_for_index(composer.state.cursor()),
                        bounds.top() + px(10.),
                    ),
                    size(px(1.5), px(22.)),
                ),
                rgb(0x2563eb),
            )
        });
        ComposerPrepaint {
            editable_line,
            placeholder_line,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.composer.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.composer.clone()),
            cx,
        );
        if let Some(selection) = state.selection.take() {
            window.paint_quad(selection);
        }
        if let Some(placeholder_line) = &state.placeholder_line {
            placeholder_line
                .paint(
                    point(bounds.left(), bounds.top() + px(10.)),
                    px(22.),
                    window,
                    cx,
                )
                .expect("placeholder paint");
        } else {
            state
                .editable_line
                .paint(
                    point(bounds.left(), bounds.top() + px(10.)),
                    px(22.),
                    window,
                    cx,
                )
                .expect("text paint");
        }
        if focus.is_focused(window)
            && let Some(cursor) = state.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.composer.update(cx, |composer, _| {
            composer.last_layout = Some(state.editable_line.clone());
            composer.last_bounds = Some(bounds);
        });
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Composer")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::insert_newline))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .w_full()
            .child(ComposerElement {
                composer: cx.entity(),
            })
    }
}

#[derive(Debug)]
struct TranscriptScroll {
    handle: ScrollHandle,
    follow_tail: bool,
}

impl TranscriptScroll {
    fn new() -> Self {
        Self {
            handle: ScrollHandle::new(),
            follow_tail: true,
        }
    }

    fn observe(&mut self) {
        let offset = self.handle.offset().y;
        let max = self.handle.max_offset().height;
        self.follow_tail = (offset + max).abs() <= px(1.0);
    }

    #[cfg(test)]
    fn content_changed(&self) {
        if self.follow_tail {
            self.handle.scroll_to_bottom();
        }
    }
}

struct DeepChatShell {
    composer: Entity<Composer>,
    transcript: TranscriptScroll,
}

impl Render for DeepChatShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.transcript.observe();
        let shell = cx.entity().downgrade();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xf7f7f8))
            .text_color(rgb(0x202124))
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .border_b_1()
                    .border_color(rgb(0xe4e4e7))
                    .child(
                        div()
                            .pl(px(78.))
                            .pt(px(18.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("DeepChat"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(240.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .justify_between()
                            .border_r_1()
                            .border_color(rgb(0xe4e4e7))
                            .bg(rgb(0xf1f1f3))
                            .p(px(16.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .rounded(px(8.))
                                            .bg(rgb(0xffffff))
                                            .border_1()
                                            .border_color(rgb(0xd4d4d8))
                                            .p(px(10.))
                                            .child("+ New chat"),
                                    )
                                    .child(div().p(px(10.)).child("Search"))
                                    .child(
                                        div()
                                            .mt(px(18.))
                                            .text_color(rgb(0x71717a))
                                            .child("No chats yet"),
                                    ),
                            )
                            .child(div().p(px(10.)).child("Settings")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id("transcript")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.transcript.handle)
                                    .on_scroll_wheel(move |_, _, cx| {
                                        if let Some(shell) = shell.upgrade() {
                                            shell.update(cx, |view, cx| {
                                                view.transcript.observe();
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .text_size(px(28.))
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .child("DeepChat"),
                                            )
                                            .child(
                                                div()
                                                    .text_color(rgb(0x71717a))
                                                    .child("Start a conversation"),
                                            ),
                                    ),
                            )
                            .child(
                                div().flex_none().p(px(18.)).pt(px(8.)).child(
                                    div()
                                        .rounded(px(14.))
                                        .border_1()
                                        .border_color(rgb(0xd4d4d8))
                                        .bg(rgb(0xffffff))
                                        .p(px(12.))
                                        .child(self.composer.clone())
                                        .child(
                                            div()
                                                .mt(px(8.))
                                                .flex()
                                                .justify_between()
                                                .text_size(px(12.))
                                                .text_color(rgb(0x71717a))
                                                .child("DeepChat · Agent mode")
                                                .child(
                                                    div()
                                                        .rounded(px(8.))
                                                        .bg(rgb(0xe4e4e7))
                                                        .px(px(10.))
                                                        .py(px(6.))
                                                        .child("Send unavailable"),
                                                ),
                                        ),
                                ),
                            ),
                    ),
            )
    }
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Composer")),
        KeyBinding::new("delete", Delete, Some("Composer")),
        KeyBinding::new("left", Left, Some("Composer")),
        KeyBinding::new("right", Right, Some("Composer")),
        KeyBinding::new("shift-left", SelectLeft, Some("Composer")),
        KeyBinding::new("shift-right", SelectRight, Some("Composer")),
        KeyBinding::new("cmd-a", SelectAll, Some("Composer")),
        KeyBinding::new("cmd-c", Copy, Some("Composer")),
        KeyBinding::new("cmd-v", Paste, Some("Composer")),
        KeyBinding::new("cmd-x", Cut, Some("Composer")),
        KeyBinding::new("cmd-z", Undo, Some("Composer")),
        KeyBinding::new("cmd-shift-z", Redo, Some("Composer")),
        KeyBinding::new("enter", Submit, Some("Composer")),
        KeyBinding::new("shift-enter", InsertNewline, Some("Composer")),
        KeyBinding::new("home", Home, Some("Composer")),
        KeyBinding::new("end", End, Some("Composer")),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

fn main() {
    Application::new().run(|cx: &mut App| {
        bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let bounds = Bounds::centered(None, size(px(800.), px(620.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("DeepChat".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.), px(18.))),
                    }),
                    window_min_size: None,
                    ..Default::default()
                },
                |_, cx| {
                    let composer = cx.new(Composer::new);
                    cx.new(|_| DeepChatShell {
                        composer,
                        transcript: TranscriptScroll::new(),
                    })
                },
            )
            .expect("open DeepChat window");
        window
            .update(cx, |shell, window, cx| {
                window.focus(&shell.composer.read(cx).focus);
                cx.activate(true);
            })
            .expect("focus composer");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_utf8_and_utf16_without_splitting_surrogate_pairs() {
        let state = ComposerState {
            text: "A😀中".into(),
            ..Default::default()
        };
        assert_eq!(state.offset_to_utf16(1), 1);
        assert_eq!(state.offset_to_utf16(5), 3);
        assert_eq!(state.offset_from_utf16(2), 1);
        assert_eq!(state.range_from_utf16(1..3), 1..5);
        assert_eq!(state.range_to_utf16(1..8), 1..4);
    }

    #[test]
    fn placeholder_hit_testing_always_stays_in_the_empty_editable_domain() {
        assert_eq!(editable_index_for_hit("", 0), 0);
        assert_eq!(editable_index_for_hit("", "Message DeepChat".len()), 0);

        let mut state = ComposerState::default();
        state.move_to(editable_index_for_hit(&state.text, 7));
        state.replace(None, "x");
        assert_eq!(state.text, "x");
        assert_eq!(state.selection, 1..1);
    }

    #[test]
    fn marked_text_replaces_requested_range_and_tracks_relative_selection() {
        let mut state = ComposerState {
            text: "hello 世界".into(),
            selection: 5..5,
            ..Default::default()
        };
        state.replace_and_mark(Some(6..8), "😀文", Some(2..3));
        assert_eq!(state.text, "hello 😀文");
        assert_eq!(state.marked_range, Some(6..13));
        assert_eq!(state.selection, 10..13);
        state.replace(None, "字");
        assert_eq!(state.text, "hello 字");
        assert_eq!(state.marked_range, None);
    }

    #[test]
    fn ime_composition_updates_coalesce_into_one_undo_unit() {
        let mut state = ComposerState {
            text: "start ".into(),
            selection: 6..6,
            ..Default::default()
        };
        state.replace_and_mark(None, "n", Some(1..1));
        state.replace_and_mark(None, "ni", Some(2..2));
        state.replace_and_mark(None, "你", Some(1..1));
        state.unmark();

        assert_eq!(state.text, "start 你");
        assert_eq!(state.undo.len(), 1);
        state.undo();
        assert_eq!(state.text, "start ");
        assert_eq!(state.selection, 6..6);
    }

    #[test]
    fn empty_marked_text_still_prevents_submit_until_composition_ends() {
        let mut state = ComposerState {
            text: "draft".into(),
            selection: 5..5,
            ..Default::default()
        };
        state.replace_and_mark(None, "", Some(0..0));

        assert_eq!(state.marked_range, None);
        assert!(state.composition_snapshot.is_some());
        assert!(!state.submit());
        assert_eq!(state.submit_attempts, 0);
        assert_eq!(state.text, "draft");

        state.unmark();
        assert!(!state.submit());
        assert_eq!(state.submit_attempts, 1);
        assert_eq!(state.text, "draft");
    }

    #[test]
    fn newline_visualization_preserves_editable_byte_indices_and_ranges() {
        let text = "one\n😀\n中";
        let display = composer_display_text(text);
        assert_eq!(display.len(), text.len());
        assert_eq!(display.as_bytes()[3], b' ');
        assert_eq!(display.as_bytes()[8], b' ');

        let state = ComposerState {
            text: text.into(),
            selection: 4..8,
            marked_range: Some(9..12),
            ..Default::default()
        };
        assert_eq!(state.range_to_utf16(state.selection.clone()), 4..6);
        assert_eq!(
            state.range_to_utf16(state.marked_range.clone().unwrap()),
            7..8
        );
        assert_eq!(editable_index_for_hit(&state.text, 6), 4);
    }

    #[test]
    fn newline_clipboard_style_edits_and_undo_redo_preserve_unicode() {
        let mut state = ComposerState {
            text: "one😀".into(),
            selection: 3..3,
            ..Default::default()
        };
        state.replace(None, "\n");
        assert_eq!(state.text, "one\n😀");
        state.selection = 0..3;
        state.replace(None, "two");
        assert_eq!(state.text, "two\n😀");
        state.undo();
        assert_eq!(state.text, "one\n😀");
        state.redo();
        assert_eq!(state.text, "two\n😀");
    }

    #[test]
    fn long_fixture_updates_do_not_move_transcript_after_leaving_tail() {
        let transcript = TranscriptScroll::new();
        let mut fixture_rows = (0..200)
            .map(|index| format!("fixture row {index}"))
            .collect::<Vec<_>>();
        transcript.handle.set_offset(point(px(0.), px(-120.)));
        let before = transcript.handle.offset();
        let mut transcript = TranscriptScroll {
            follow_tail: false,
            ..transcript
        };

        fixture_rows.push("streaming-style fixture update".into());
        transcript.content_changed();

        assert_eq!(fixture_rows.len(), 201);
        assert_eq!(transcript.handle.offset(), before);
        transcript.follow_tail = true;
        transcript.content_changed();
        assert_eq!(
            transcript.handle.offset(),
            before,
            "scroll request is deferred until layout"
        );
    }

    #[test]
    fn cut_copy_and_paste_replacement_semantics_preserve_unicode() {
        let mut state = ComposerState {
            text: "copy 😀 here".into(),
            selection: 5..9,
            ..Default::default()
        };
        let clipboard = state.text[state.selection.clone()].to_owned();
        assert_eq!(clipboard, "😀");

        state.replace(None, "");
        assert_eq!(state.text, "copy  here");
        state.selection = 5..5;
        state.replace(None, &clipboard);
        assert_eq!(state.text, "copy 😀 here");
    }
}
