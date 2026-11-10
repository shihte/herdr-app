//! SiaoDesk: real web pages — a WKWebView (through wry, as gpui-kit's gpui-wry does), not pictures of a
//! headless browser. Anything whose content is the `SiaoWeb` mod (props `{url, title}`) is a page: a tab
//! pane (a browser tab; its header, SiaoPaneHead, is the toolbar) or a layout block (預覽 in the right
//! column; the SiaoWeb mod draws the toolbar on top and the page fills the rest). The props follow the
//! page so the toolbar can show the address. Commands: `web.open {pane?, url}`, `web.back`, `web.forward`, `web.reload`.
//!
//! The page is a native view above the window's own drawing, so it covers whatever GPUI draws in its
//! place. While a dialog, popup, menu or full-window overlay is up (the quick guide, the drawer drag),
//! every page is hidden; so is a page whose pane wasn't drawn last frame
//! (another tab, a zoomed neighbour).

use super::*;
use std::cell::Cell;
use std::rc::Rc;
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebViewBuilder, dpi};

pub const MOD: &str = "SiaoWeb";
const HOME: &str = "https://siao.ai";

/// The home page from 設定 → SiaoDesk (shared "siao.settings" home).
fn home() -> String {
    mods::shared("siao.settings").and_then(|s| s.get("home").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| HOME.to_owned())
}

pub struct Web {
    view: Rc<wry::WebView>,
    shown: Rc<Cell<bool>>,
    drawn: Rc<Cell<u64>>,
}

/// What a page tells the window (from wry's callbacks, which have no window context).
pub enum WebEvent {
    Url(String, String),
    Title(String, String),
    Open(String, String),
}

impl AppWindow {
    /// The web panes that should exist: every SiaoWeb pane in every tab (a page keeps its place and
    /// history while its tab is in the background).
    fn web_panes(&self) -> Vec<(String, String)> {
        let page = |p: &Node| match p.content() {
            Some(Content::Mod { name, props }) if name == MOD => {
                Some((p.id.clone(), props.get("url").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(home)))
            }
            _ => None,
        };
        // Layout blocks only while shown (a hidden 預覽 needn't load); tab panes always.
        let mut out: Vec<_> = self.chrome.visible_blocks().into_iter().filter_map(page).collect();
        for (id, url) in self.model.tabs().flat_map(|c| c.root.panes()).filter_map(page) {
            if !out.iter().any(|(o, _)| *o == id) { out.push((id, url)); }
        }
        // A page made for a block that's hidden now is kept (its history too).
        for id in self.webs.keys() {
            if !out.iter().any(|(o, _)| o == id) && self.chrome.find(id).is_some() { out.push((id.clone(), String::new())); }
        }
        out
    }

    /// Makes a page for each new web pane and drops those whose pane is gone (from `sync`).
    pub(super) fn web_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = self.web_panes();
        self.webs.retain(|id, w| {
            let keep = want.iter().any(|(p, _)| p == id);
            if !keep {
                let _ = w.view.set_visible(false);
            }
            keep
        });
        for (pane, url) in want {
            if self.webs.contains_key(&pane) {
                continue;
            }
            match self.web_build(&pane, &url, window, cx) {
                Ok(view) => {
                    self.webs.insert(pane, Web { view: Rc::new(view), shown: Rc::new(Cell::new(true)), drawn: Rc::new(Cell::new(0)) });
                }
                Err(e) => tracing::warn!("web pane {pane}: {e}"),
            }
        }
    }

    fn web_build(&mut self, pane: &str, url: &str, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<wry::WebView> {
        use wry::raw_window_handle::HasWindowHandle;
        let tx = self.web_events(window, cx);
        let (on_load, on_title, on_open) = (tx.clone(), tx.clone(), tx);
        let (p1, p2, p3) = (pane.to_owned(), pane.to_owned(), pane.to_owned());
        let handle = HasWindowHandle::window_handle(&*window).map_err(|e| anyhow::anyhow!("no window handle: {e}"))?;
        let view = WebViewBuilder::new()
            .with_url(url)
            .with_on_page_load_handler(move |ev, url| {
                if matches!(ev, PageLoadEvent::Started | PageLoadEvent::Finished) {
                    let _ = on_load.try_send(WebEvent::Url(p1.clone(), url));
                }
            })
            .with_document_title_changed_handler(move |title| { let _ = on_title.try_send(WebEvent::Title(p2.clone(), title)); })
            // Links that want a new window open here: the pane shows one page.
            .with_new_window_req_handler(move |url, _| { let _ = on_open.try_send(WebEvent::Open(p3.clone(), url)); NewWindowResponse::Deny })
            .with_bounds(Rect::default())
            .build_as_child(&handle)?;
        round_corners(&view);
        Ok(view)
    }

    /// One channel for every page's callbacks, drained on the window's thread.
    fn web_events(&mut self, window: &mut Window, cx: &mut Context<Self>) -> async_channel::Sender<WebEvent> {
        if let Some(tx) = &self.web_tx {
            return tx.clone();
        }
        let (tx, rx) = async_channel::unbounded::<WebEvent>();
        self.web_tx = Some(tx.clone());
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                let _ = this.update(cx, |this, cx| this.web_event(ev, cx));
            }
        }).detach();
        tx
    }

    fn web_event(&mut self, ev: WebEvent, cx: &mut Context<Self>) {
        let (pane, key, value) = match ev {
            WebEvent::Url(p, u) => (p, "url", u),
            WebEvent::Title(p, t) => (p, "title", t),
            WebEvent::Open(p, u) => {
                if let Some(w) = self.webs.get(&p) { let _ = w.view.load_url(&u); }
                return;
            }
        };
        let Some(n) = self.find_node_mut(&pane) else { return };
        if let Kind::Block { content: Content::Mod { props, .. } } = &mut n.kind {
            if !props.is_object() { *props = json!({}); }
            if props.get(key).and_then(Value::as_str) == Some(value.as_str()) { return; }
            props[key] = json!(value);
        }
        self.publish(cx);
        self.save();
    }

    /// Hides the pages that mustn't show this frame (from `render`, before the tree is built).
    pub(super) fn web_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.web_frame_no += 1;
        // A page shown since the last sync (預覽 picked in the right column) gets made now.
        if self.web_panes().iter().any(|(id, _)| !self.webs.contains_key(id)) {
            self.web_sync(window, cx);
        }
        let cover = self.web_covered();
        for w in self.webs.values() {
            if (cover || w.drawn.get() + 1 < self.web_frame_no) && w.shown.get() {
                let _ = w.view.set_visible(false);
                w.shown.set(false);
            }
        }
    }

    fn web_covered(&self) -> bool {
        // Resizing (a divider dragged, a glide) never hides it: it follows its box every frame.
        self.overlays.iter().any(|o| !matches!(o.kind, OverlayKind::Toast))
    }

    /// The page in pane `id`'s place: a measured box the native view is moved onto.
    pub(super) fn render_web(&self, id: &str) -> AnyElement {
        let Some(w) = self.webs.get(id) else {
            return div().size_full().bg(c("background")).into_any_element();
        };
        if self.web_covered() {
            return div().size_full().bg(c("background")).into_any_element();
        }
        let (view, shown, drawn, frame) = (w.view.clone(), w.shown.clone(), w.drawn.clone(), self.web_frame_no);
        let blur = w.view.clone();
        // A frame of the pane's own background round the page, so it never runs into the pane's border
        // or the toolbar's line (its corners are rounded natively, round_corners).
        // The same line under the toolbar for a browser tab and for 預覽 (the toolbar draws none).
        div().size_full().border_t(px(1.)).border_color(c("border")).p(settings::z(6.)).child(
            canvas(
                move |bounds, _, _| {
                    let _ = view.set_bounds(Rect {
                        size: dpi::Size::Logical(dpi::LogicalSize { width: f32::from(bounds.size.width) as f64, height: f32::from(bounds.size.height) as f64 }),
                        position: dpi::Position::Logical(dpi::LogicalPosition::new(f32::from(bounds.origin.x) as f64, f32::from(bounds.origin.y) as f64)),
                    });
                    if !shown.get() {
                        let _ = view.set_visible(true);
                        shown.set(true);
                    }
                    drawn.set(frame);
                    bounds
                },
                move |_, bounds, window, _| {
                    // A click anywhere else gives the keyboard back to the window.
                    let blur = blur.clone();
                    window.on_mouse_event(move |ev: &MouseDownEvent, _, _, _| {
                        if !bounds.contains(&ev.position) {
                            let _ = blur.focus_parent();
                        }
                    });
                },
            )
            .size_full(),
        ).into_any_element()
    }

    /// web.open / web.back / web.forward / web.reload.
    pub(super) fn web_command(&mut self, name: &str, a: &Value) -> Result<(), String> {
        let pane = Self::owned(a, "pane")
            .or_else(|| self.model.focused_pane().filter(|p| self.webs.contains_key(p)))
            .or_else(|| (self.webs.len() == 1).then(|| self.webs.keys().next().cloned()).flatten())
            .ok_or("no 預覽 pane (give pane)")?;
        let w = self.webs.get(&pane).ok_or_else(|| format!("pane {pane:?} shows no page"))?;
        let r = match name {
            "web.open" => {
                let url = Self::arg(a, "url").ok_or("web.open needs url")?;
                w.view.load_url(&web_url(url))
            }
            "web.back" => w.view.evaluate_script("history.back()"),
            "web.forward" => w.view.evaluate_script("history.forward()"),
            _ => w.view.reload(),
        };
        r.map_err(|e| e.to_string())
    }
}

/// The page sits as a card inset in its pane (render_web) and GPUI can't clip a native view: round
/// its own corners, all four.
fn round_corners(view: &wry::WebView) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use wry::WebViewExtMacOS;
    let wk = view.webview();
    unsafe {
        let wk: &AnyObject = &*(objc2::rc::Retained::as_ptr(&wk) as *const AnyObject);
        let _: () = msg_send![wk, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![wk, layer];
        if let Some(layer) = layer.as_ref() {
            let _: () = msg_send![layer, setCornerRadius: 7.0f64];
            let _: () = msg_send![layer, setMasksToBounds: true];
            // All four (kCALayerMinXMinYCorner | MaxXMinY | MinXMaxY | MaxXMaxY).
            let _: () = msg_send![layer, setMaskedCorners: 15usize];
        }
    }
}

/// What someone typed in the address box, as a URL: a scheme as is, a path as a file, localhost and
/// anything with a dot as a site, the rest as a search.
fn web_url(raw: &str) -> String {
    let s = raw.trim();
    if s.contains("://") || s.starts_with("about:") || s.starts_with("data:") {
        return s.to_owned();
    }
    if s.starts_with('/') {
        return format!("file://{s}");
    }
    if s.starts_with("localhost") || s.starts_with("127.") || s.starts_with("0.0.0.0") {
        return format!("http://{s}");
    }
    if !s.contains(' ') && s.contains('.') {
        return format!("https://{s}");
    }
    let q: String = s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "+".to_owned(),
        _ => format!("%{b:02X}"),
    }).collect();
    format!("https://www.google.com/search?q={q}")
}
