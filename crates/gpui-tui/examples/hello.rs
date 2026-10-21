//! A small interactive GPUI view in the terminal: hover, click, scroll, and key input.
//!
//! Run inside kitty or Ghostty: `cargo run -p gpui-tui --example hello`

use gpui::{
    App, Context, FocusHandle, KeyDownEvent, MouseButton, ScrollWheelEvent, Window, WindowOptions,
    div, prelude::*, px, rgb,
};

struct Hello {
    focus_handle: FocusHandle,
    clicks: usize,
    last_key: String,
    scrolled: f32,
}

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.last_key = event.keystroke.to_string();
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.scrolled += f32::from(event.delta.pixel_delta(window.line_height()).y);
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .justify_center()
            .items_center()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .text_xl()
            .child("Hello from GPUI, inside a terminal")
            .child(
                div()
                    .id("counter")
                    .px_6()
                    .py_3()
                    .rounded_lg()
                    .shadow_md()
                    .bg(rgb(0x89b4fa))
                    .text_color(rgb(0x1e1e2e))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xb4befe)))
                    .active(|style| style.bg(rgb(0x74c7ec)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clicks += 1;
                        cx.notify();
                    }))
                    .child(format!("Clicked {} times", self.clicks)),
            )
            .child(
                div().flex().gap_2().children(
                    [0xf38ba8, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x89dceb]
                        .into_iter()
                        .map(|color| div().size(px(32.0)).rounded_md().bg(rgb(color))),
                ),
            )
            .child(format!("Last key: {}", self.last_key))
            .child(format!("Scrolled: {:.0}px", self.scrolled))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x9399b2))
                    .child("Press Ctrl+C to quit"),
            )
    }
}

fn main() {
    gpui_tui::application().run(|cx: &mut App| {
        let opened = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                window.focus(&focus_handle, cx);
                Hello {
                    focus_handle,
                    clicks: 0,
                    last_key: "none".to_owned(),
                    scrolled: 0.0,
                }
            })
        });
        if let Err(error) = opened {
            log::error!("failed to open the window: {error:#}");
            cx.quit();
        }
        cx.activate(true);
    });
}
