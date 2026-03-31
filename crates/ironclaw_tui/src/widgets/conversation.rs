//! Conversation widget: renders chat messages with basic markdown.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::layout::TuiSlot;
use unicode_width::UnicodeWidthStr;

use crate::render::{format_tool_duration, render_markdown, truncate, wrap_text};
use crate::theme::Theme;

use super::{AppState, MessageRole, ToolActivity, ToolStatus, TuiWidget};

pub struct ConversationWidget {
    theme: Theme,
}

impl ConversationWidget {
    pub fn new(theme: Theme) -> Self {
        Self { theme }
    }
}

impl TuiWidget for ConversationWidget {
    fn id(&self) -> &str {
        "conversation"
    }

    fn slot(&self) -> TuiSlot {
        TuiSlot::Tab
    }

    fn render(&self, area: Rect, buf: &mut Buffer, state: &AppState) {
        if area.height == 0 || area.width < 4 {
            return;
        }

        let usable_width = (area.width as usize).saturating_sub(4);
        let mut all_lines: Vec<Line<'_>> = Vec::new();

        for msg in &state.messages {
            let (prefix, style) = match msg.role {
                MessageRole::User => ("\u{25CF} ", self.theme.accent_style()),
                MessageRole::Assistant => ("", Style::default().fg(self.theme.fg.to_color())),
                MessageRole::System => ("\u{25CB} ", self.theme.dim_style()),
            };

            if msg.role == MessageRole::User {
                // Blank line before user messages (except first)
                if !all_lines.is_empty() {
                    all_lines.push(Line::from(""));
                }
                let user_line = Line::from(vec![
                    Span::styled(prefix.to_string(), self.theme.accent_style()),
                    Span::styled(msg.content.clone(), self.theme.bold_style()),
                ]);
                all_lines.push(user_line);
                all_lines.push(Line::from(""));
            } else if msg.role == MessageRole::Assistant {
                // Separator before assistant response
                let sep = "\u{2500}".repeat(usable_width.min(60));
                all_lines.push(Line::from(Span::styled(
                    format!("  {sep}"),
                    self.theme.dim_style(),
                )));

                let wrapped =
                    render_markdown(&msg.content, usable_width.saturating_sub(2), &self.theme);
                for line in wrapped {
                    let mut padded = vec![Span::raw("  ".to_string())];
                    padded.extend(
                        line.spans
                            .into_iter()
                            .map(|s| Span::styled(s.content.to_string(), s.style)),
                    );
                    all_lines.push(Line::from(padded));
                }
                all_lines.push(Line::from(""));
            } else {
                let wrapped = wrap_text(&msg.content, usable_width, style);
                all_lines.extend(wrapped);
            }
        }

        // Inline tool calls (current turn only: tools started after last assistant message)
        let last_assistant_ts = state
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
            .map(|m| m.timestamp);

        let turn_recent: Vec<&ToolActivity> = state
            .recent_tools
            .iter()
            .filter(|t| match last_assistant_ts {
                Some(ts) => t.started_at > ts,
                None => true,
            })
            .collect();

        if !turn_recent.is_empty() || !state.active_tools.is_empty() {
            all_lines.push(Line::from(""));
            for tool in &turn_recent {
                all_lines.push(self.render_tool_line(tool, usable_width, false));
            }
            for tool in &state.active_tools {
                all_lines.push(self.render_tool_line(tool, usable_width, true));
            }
        }

        // Show thinking indicator if active
        if !state.status_text.is_empty() && !state.is_streaming {
            all_lines.push(Line::from(Span::styled(
                format!("  \u{25CB} {}", state.status_text),
                self.theme.dim_style(),
            )));
        }

        // Compute visible window (scroll from bottom)
        let visible_height = area.height as usize;
        let total_lines = all_lines.len();
        let scroll = state.scroll_offset as usize;
        let start = total_lines.saturating_sub(visible_height + scroll);
        let end = total_lines.saturating_sub(scroll).min(total_lines);

        let visible: Vec<Line<'_>> = all_lines
            .into_iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();

        let paragraph = ratatui::widgets::Paragraph::new(visible);
        paragraph.render(area, buf);
    }
}

impl ConversationWidget {
    /// Render a single tool call line in the Claude Code inline style.
    ///
    /// Format: `  ┊ icon $  command_text...             1.3s`
    fn render_tool_line(
        &self,
        tool: &ToolActivity,
        usable_width: usize,
        is_active: bool,
    ) -> Line<'static> {
        let (icon, icon_style) = if is_active {
            ("\u{25CB}", self.theme.accent_style()) // ○ running
        } else {
            match tool.status {
                ToolStatus::Success => ("\u{25CF}", self.theme.success_style()), // ● green
                ToolStatus::Failed => ("\u{2717}", self.theme.error_style()),    // ✗ red
                ToolStatus::Running => ("\u{25CB}", self.theme.accent_style()),  // ○ accent
            }
        };

        // Duration text
        let duration_text = if is_active {
            let elapsed = chrono::Utc::now()
                .signed_duration_since(tool.started_at)
                .num_milliseconds()
                .unsigned_abs();
            format_tool_duration(elapsed)
        } else {
            tool.duration_ms
                .map(format_tool_duration)
                .unwrap_or_default()
        };

        // Build the command description: "$ detail" or "tool_name detail"
        let cmd_text = match &tool.detail {
            Some(d) => format!("$  {d}"),
            None => format!("$  {}", tool.name),
        };

        // Layout: "  ┊ icon  cmd...  duration"
        //          ^2  ^2    ^cmd    ^gap ^duration
        let prefix = format!("  \u{250A} {icon} ");
        let prefix_width = UnicodeWidthStr::width(prefix.as_str());
        let duration_width = UnicodeWidthStr::width(duration_text.as_str());
        let available_for_cmd =
            usable_width.saturating_sub(prefix_width + duration_width + 2); // 2 for gap

        let cmd_truncated = truncate(&cmd_text, available_for_cmd);
        let cmd_width = UnicodeWidthStr::width(cmd_truncated.as_str());

        // Pad between command and duration
        let gap = usable_width
            .saturating_sub(prefix_width + cmd_width + duration_width)
            .max(1);
        let padding = " ".repeat(gap);

        Line::from(vec![
            Span::styled("  \u{250A} ".to_string(), self.theme.dim_style()),
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(cmd_truncated, self.theme.dim_style()),
            Span::raw(padding),
            Span::styled(duration_text, self.theme.dim_style()),
        ])
    }

    /// Handle scroll up/down. Returns true if scrolling occurred.
    pub fn scroll(&self, state: &mut AppState, delta: i16) {
        if delta < 0 {
            state.scroll_offset = state.scroll_offset.saturating_add(delta.unsigned_abs());
        } else {
            state.scroll_offset = state.scroll_offset.saturating_sub(delta as u16);
        }
    }
}
