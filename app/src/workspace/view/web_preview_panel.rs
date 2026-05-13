use crate::appearance::Appearance;
use crate::editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions};
use crate::pane_group::pane::view::header::PANE_HEADER_HEIGHT;
use crate::view_components::action_button::{ActionButton, PaneHeaderTheme};
use crate::workspace::action::WorkspaceAction;
use warp_core::ui::Icon;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, Radius, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, Entity, TypedActionView, View, ViewContext, ViewHandle};

const URL_BAR_HEIGHT: f32 = 32.;
const DEFAULT_URL: &str = "https://";

#[derive(Clone, Debug)]
pub enum WebPreviewPanelAction {
    Navigate,
    Back,
    Forward,
    OpenInBrowser,
}

pub struct WebPreviewPanelView {
    url_editor: ViewHandle<EditorView>,
    back_button: ViewHandle<ActionButton>,
    forward_button: ViewHandle<ActionButton>,
    open_external_button: ViewHandle<ActionButton>,
    close_button: ViewHandle<ActionButton>,
    history: Vec<String>,
    history_pos: usize,
}

impl WebPreviewPanelView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let url_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_text(Some(appearance.ui_font_size()), appearance),
                    select_all_on_focus: true,
                    clear_selections_on_blur: true,
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text(DEFAULT_URL, ctx);
            editor
        });

        ctx.subscribe_to_view(&url_editor, |me, editor_view, event, ctx| match event {
            EditorEvent::Enter => {
                let url = editor_view.as_ref(ctx).buffer_text(ctx);
                me.navigate(url.trim().to_string(), ctx);
            }
            EditorEvent::Escape => {
                ctx.dispatch_typed_action(WorkspaceAction::ToggleWebPreviewPanel);
            }
            _ => {}
        });

        let back_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("", PaneHeaderTheme)
                .with_icon(Icon::ChevronLeft)
                .with_tooltip("Back")
                .on_click(|ctx| ctx.dispatch_typed_action(WebPreviewPanelAction::Back))
        });

        let forward_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("", PaneHeaderTheme)
                .with_icon(Icon::ChevronRight)
                .with_tooltip("Forward")
                .on_click(|ctx| ctx.dispatch_typed_action(WebPreviewPanelAction::Forward))
        });

        let open_external_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Open in Browser", PaneHeaderTheme)
                .with_tooltip("Open current URL in system browser")
                .on_click(|ctx| ctx.dispatch_typed_action(WebPreviewPanelAction::OpenInBrowser))
        });

        let close_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("", PaneHeaderTheme)
                .with_icon(Icon::X)
                .with_tooltip("Close web preview")
                .on_click(|ctx| ctx.dispatch_typed_action(WorkspaceAction::ToggleWebPreviewPanel))
        });

        Self {
            url_editor,
            back_button,
            forward_button,
            open_external_button,
            close_button,
            history: vec![DEFAULT_URL.to_string()],
            history_pos: 0,
        }
    }

    fn current_url(&self) -> &str {
        self.history
            .get(self.history_pos)
            .map(|s| s.as_str())
            .unwrap_or(DEFAULT_URL)
    }

    pub(crate) fn navigate(&mut self, url: String, ctx: &mut ViewContext<Self>) {
        if url.is_empty() || url == DEFAULT_URL {
            return;
        }
        let url = if !url.contains("://") {
            format!("https://{url}")
        } else {
            url
        };
        // Truncate forward history when navigating to a new URL
        self.history.truncate(self.history_pos + 1);
        self.history.push(url.clone());
        self.history_pos = self.history.len() - 1;

        // Sync editor to normalized URL
        let url_clone = url.clone();
        self.url_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&url_clone, ctx);
        });

        ctx.open_url(&url);
        ctx.notify();
    }

    fn go_back(&mut self, ctx: &mut ViewContext<Self>) {
        if self.history_pos == 0 {
            return;
        }
        self.history_pos -= 1;
        let url = self.history[self.history_pos].clone();
        self.url_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&url, ctx);
        });
        ctx.open_url(&url);
        ctx.notify();
    }

    fn go_forward(&mut self, ctx: &mut ViewContext<Self>) {
        if self.history_pos + 1 >= self.history.len() {
            return;
        }
        self.history_pos += 1;
        let url = self.history[self.history_pos].clone();
        self.url_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&url, ctx);
        });
        ctx.open_url(&url);
        ctx.notify();
    }

    fn render_header(&self, appearance: &Appearance, _app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let sub_color = theme.sub_text_color(theme.background());

        let title = Text::new_inline(
            "Web Preview",
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(sub_color.into())
        .with_selectable(false)
        .finish();

        let close_btn = ConstrainedBox::new(ChildView::new(&self.close_button).finish())
            .with_width(24.)
            .with_height(24.)
            .finish();

        ConstrainedBox::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(Container::new(title).with_margin_left(16.).finish())
                    .with_child(Container::new(close_btn).with_margin_right(8.).finish())
                    .finish(),
            )
            .finish(),
        )
        .with_height(PANE_HEADER_HEIGHT)
        .finish()
    }

    fn render_address_bar(&self, appearance: &Appearance, _app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();

        let back_btn = ConstrainedBox::new(ChildView::new(&self.back_button).finish())
            .with_width(24.)
            .with_height(24.)
            .finish();

        let forward_btn = ConstrainedBox::new(ChildView::new(&self.forward_button).finish())
            .with_width(24.)
            .with_height(24.)
            .finish();

        let url_input = Shrinkable::new(
            1.0,
            Container::new(
                Shrinkable::new(1.0, ChildView::new(&self.url_editor).finish()).finish(),
            )
            .with_background_color(theme.surface_1().into_solid())
            .with_horizontal_padding(8.)
            .with_vertical_padding(4.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish(),
        )
        .finish();

        ConstrainedBox::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_child(back_btn)
                    .with_child(forward_btn)
                    .with_child(Container::new(url_input).with_margin_left(4.).with_margin_right(8.).finish())
                    .finish(),
            )
            .with_horizontal_padding(8.)
            .finish(),
        )
        .with_height(URL_BAR_HEIGHT)
        .finish()
    }

    fn render_content_area(&self, appearance: &Appearance, _app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let sub_color = theme.sub_text_color(theme.background());
        let current_url = self.current_url().to_string();

        let title_text = Text::new_inline(
            "Web Preview",
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(sub_color.into())
        .with_selectable(false)
        .finish();

        let hint_text = Text::new_inline(
            "Type a URL in the address bar and press Enter.",
            appearance.ui_font_family(),
            appearance.ui_font_size() - 1.,
        )
        .with_color(sub_color.into())
        .with_selectable(false)
        .finish();

        let open_btn = ChildView::new(&self.open_external_button).finish();

        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_alignment(MainAxisAlignment::Center);
        col.add_child(Container::new(title_text).with_margin_bottom(8.).finish());
        col.add_child(Container::new(hint_text).with_margin_bottom(12.).finish());

        if current_url != DEFAULT_URL {
            let url_text = Text::new_inline(
                current_url,
                appearance.ui_font_family(),
                appearance.ui_font_size() - 1.,
            )
            .with_color(sub_color.into())
            .with_selectable(true)
            .finish();
            col.add_child(Container::new(url_text).with_margin_bottom(12.).finish());
        }

        col.add_child(open_btn);

        Shrinkable::new(
            1.0,
            Container::new(col.finish())
                .with_uniform_padding(16.)
                .finish(),
        )
        .finish()
    }
}

impl Entity for WebPreviewPanelView {
    type Event = ();
}

impl TypedActionView for WebPreviewPanelView {
    type Action = WebPreviewPanelAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            WebPreviewPanelAction::Navigate => {
                let url = self.url_editor.as_ref(ctx).buffer_text(ctx);
                self.navigate(url.trim().to_string(), ctx);
            }
            WebPreviewPanelAction::Back => self.go_back(ctx),
            WebPreviewPanelAction::Forward => self.go_forward(ctx),
            WebPreviewPanelAction::OpenInBrowser => {
                let url = self.current_url().to_string();
                if url != DEFAULT_URL {
                    ctx.open_url(&url);
                }
            }
        }
    }
}

impl View for WebPreviewPanelView {
    fn ui_name() -> &'static str {
        "WebPreviewPanelView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        Flex::column()
            .with_child(self.render_header(appearance, app))
            .with_child(self.render_address_bar(appearance, app))
            .with_child(self.render_content_area(appearance, app))
            .finish()
    }
}
