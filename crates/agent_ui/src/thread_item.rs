//! Center-pane item wrapper for an agent conversation.
//!
//! [`ConversationView`] is the surface the Agent Panel renders today. It is not
//! a workspace [`Item`], so it can only live inside the panel. This module wraps
//! it in a minimal `Item` implementation so a thread can also be opened as a
//! regular tab in the center pane, next to editors and terminals.
//!
//! Scope notes:
//! - The thread is still created through [`AgentPanel`], so it keeps its thread
//!   metadata, sidebar entry, and notification behavior.
//! - Serialization is intentionally not implemented. A center thread does not
//!   survive a workspace restore yet; reopening is a manual action.

use crate::{Agent, AgentPanel, ConversationView, NewCenterThread};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Window, prelude::*,
};
use project::DisableAiSettings;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::{
    Icon, IconName, Tooltip,
    prelude::*,
    utils::WithRemSize,
};
use workspace::{
    Item, Workspace,
    item::{ItemEvent, TabTooltipContent},
};

/// A workspace item that hosts an agent conversation in a center pane.
pub struct ThreadItem {
    conversation_view: Entity<ConversationView>,
    focus_handle: FocusHandle,
}

impl ThreadItem {
    pub fn new(conversation_view: Entity<ConversationView>, cx: &mut Context<Self>) -> Self {
        Self {
            conversation_view,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Opens a new agent thread as a tab in the current pane.
    pub fn deploy(
        workspace: &mut Workspace,
        action: &NewCenterThread,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if DisableAiSettings::get_global(cx).disable_ai {
            return;
        }
        let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
            return;
        };

        let agent = match &action.agent {
            Some(agent_id) => Agent::from(agent_id.clone()),
            None => panel.read(cx).selected_agent(cx),
        };

        let conversation_view =
            panel.update(cx, |panel, cx| panel.create_center_thread(agent, window, cx));

        let item = cx.new(|cx| Self::new(conversation_view, cx));
        workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
    }
}

impl Focusable for ThreadItem {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.conversation_view.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<ItemEvent> for ThreadItem {}

impl Render for ThreadItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if DisableAiSettings::get_global(cx).disable_ai {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .track_focus(&self.focus_handle)
                .child(Label::new("AI is disabled").color(Color::Muted))
                .into_any_element();
        }

        // Mirror the Agent Panel's font sizing so a thread reads the same in a
        // center tab as it does in the panel. `WithRemSize` only implements
        // `Styled`, so focus tracking lives on an outer container.
        let theme_settings = ThemeSettings::get_global(cx);
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(
                WithRemSize::new(theme_settings.agent_ui_font_size(cx))
                    .size_full()
                    .font_family(theme_settings.agent_ui_font_family().clone())
                    .child(self.conversation_view.clone()),
            )
            .into_any_element()
    }
}

impl Item for ThreadItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.conversation_view.read(cx).title(cx)
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::ZedAgent).color(Color::Muted))
    }

    fn tab_tooltip_content(&self, cx: &App) -> Option<TabTooltipContent> {
        let title = self.conversation_view.read(cx).title(cx);
        Some(TabTooltipContent::Custom(Box::new(Tooltip::element(
            move |_window, _cx| {
                v_flex()
                    .child(Label::new(title.clone()))
                    .child(
                        Label::new("Agent Thread")
                            .color(Color::Muted)
                            .size(LabelSize::Small),
                    )
                    .into_any_element()
            },
        ))))
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("Agent Thread Opened")
    }

    fn include_in_nav_history() -> bool {
        false
    }

    fn can_split(&self) -> bool {
        true
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event);
    }
}
