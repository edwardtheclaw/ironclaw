//! Conversation widget: renders chat messages with basic markdown.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::layout::TuiSlot;
use unicode_width::UnicodeWidthStr;

use crate::render::{format_tokens, format_tool_duration, render_markdown, truncate, wrap_text};
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
        let mut turn_count = 0u32;

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
                let time_str = msg.timestamp.format("%H:%M").to_string();
                let user_line = Line::from(vec![
                    Span::styled(prefix.to_string(), self.theme.accent_style()),
                    Span::styled(msg.content.clone(), self.theme.bold_style()),
                    Span::styled(format!("  {time_str}"), self.theme.dim_style()),
                ]);
                all_lines.push(user_line);
                all_lines.push(Line::from(""));
            } else if msg.role == MessageRole::Assistant {
                // Separator with turn counter before assistant response
                turn_count += 1;
                let turn_label = format!(" Turn {turn_count} ");
                let sep_left_len = 2usize;
                let sep_right_len = usable_width
                    .min(60)
                    .saturating_sub(sep_left_len + turn_label.len());
                let sep_left = "\u{2500}".repeat(sep_left_len);
                let sep_right = "\u{2500}".repeat(sep_right_len);
                all_lines.push(Line::from(vec![
                    Span::styled(format!("  {sep_left}"), self.theme.dim_style()),
                    Span::styled(turn_label, self.theme.dim_style()),
                    Span::styled(sep_right, self.theme.dim_style()),
                ]));

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

                // Per-turn cost summary
                if let Some(ref cost) = msg.cost_summary {
                    let cost_line = format!(
                        "  \u{25CB} {}in + {}out  {}",
                        format_tokens(cost.input_tokens),
                        format_tokens(cost.output_tokens),
                        cost.cost_usd,
                    );
                    all_lines
                        .push(Line::from(Span::styled(cost_line, self.theme.dim_style())));
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
                // Tool output preview line
                if let Some(ref preview) = tool.result_preview {
                    let preview_max = usable_width.saturating_sub(8);
                    let first_line = preview.lines().next().unwrap_or("");
                    if !first_line.is_empty() {
                        all_lines.push(Line::from(vec![
                            Span::styled("  \u{250A}   ".to_string(), self.theme.dim_style()),
                            Span::styled("\u{2192} ".to_string(), self.theme.dim_style()),
                            Span::styled(
                                truncate(first_line, preview_max),
                                self.theme.dim_style(),
                            ),
                        ]));
                    }
                }
            }
            for tool in &state.active_tools {
                all_lines.push(self.render_tool_line(tool, usable_width, true));
                // Tool output preview line for active tools
                if let Some(ref preview) = tool.result_preview {
                    let preview_max = usable_width.saturating_sub(8);
                    let first_line = preview.lines().next().unwrap_or("");
                    if !first_line.is_empty() {
                        all_lines.push(Line::from(vec![
                            Span::styled("  \u{250A}   ".to_string(), self.theme.dim_style()),
                            Span::styled("\u{2192} ".to_string(), self.theme.dim_style()),
                            Span::styled(
                                truncate(first_line, preview_max),
                                self.theme.dim_style(),
                            ),
                        ]));
                    }
                }
            }
        }

        // Show thinking indicator if active
        const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

        if !state.status_text.is_empty() && !state.is_streaming {
            let frame = SPINNER[state.spinner_frame % SPINNER.len()];
            all_lines.push(Line::from(vec![
                Span::styled(format!("  {frame} "), self.theme.accent_style()),
                Span::styled(state.status_text.clone(), self.theme.dim_style()),
            ]));
        }

        // Show streaming dots indicator
        if state.is_streaming {
            let dots = match state.spinner_frame % 4 {
                0 => "·",
                1 => "··",
                2 => "···",
                _ => "",
            };
            all_lines.push(Line::from(Span::styled(
                format!("  {dots}"),
                self.theme.accent_style(),
            )));
        }

        // Render follow-up suggestions when not streaming
        if !state.suggestions.is_empty() && !state.is_streaming {
            all_lines.push(Line::from(""));
            all_lines.push(Line::from(Span::styled(
                "  Suggestions:".to_string(),
                self.theme.dim_style(),
            )));
            for (i, suggestion) in state.suggestions.iter().take(3).enumerate() {
                all_lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", i + 1), self.theme.accent_style()),
                    Span::styled(
                        truncate(suggestion, usable_width.saturating_sub(6)),
                        self.theme.dim_style(),
                    ),
                ]));
            }
        }

        // Search highlighting: replace spans that contain the query with
        // highlighted versions (black text on yellow background).
        if state.search.active && !state.search.query.is_empty() {
            let highlight_style = Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(ratatui::style::Color::Yellow);
            let query_lower = state.search.query.to_lowercase();

            all_lines = all_lines
                .into_iter()
                .map(|line| {
                    let mut new_spans: Vec<Span<'_>> = Vec::new();

                    for span in line.spans {
                        let text = span.content.to_string();
                        let text_lower = text.to_lowercase();

                        if text_lower.contains(&query_lower) {
                            let mut remaining = text.as_str();
                            while !remaining.is_empty() {
                                let lower_remaining = remaining.to_lowercase();
                                if let Some(pos) = lower_remaining.find(&query_lower) {
                                    if pos > 0 {
                                        new_spans.push(Span::styled(
                                            remaining[..pos].to_string(),
                                            span.style,
                                        ));
                                    }
                                    let match_end = pos + query_lower.len();
                                    new_spans.push(Span::styled(
                                        remaining[pos..match_end].to_string(),
                                        highlight_style,
                                    ));
                                    remaining = &remaining[match_end..];
                                } else {
                                    new_spans.push(Span::styled(
                                        remaining.to_string(),
                                        span.style,
                                    ));
                                    break;
                                }
                            }
                        } else {
                            new_spans.push(Span::styled(text, span.style));
                        }
                    }

                    Line::from(new_spans)
                })
                .collect();
        }

        // Compute visible window (scroll from bottom)
        let visible_height = area.height as usize;
        let total_lines = all_lines.len();
        let scroll = state.scroll_offset as usize;
        let start = total_lines.saturating_sub(visible_height + scroll);
        let end = total_lines.saturating_sub(scroll).min(total_lines);

        let mut visible: Vec<Line<'_>> = all_lines
            .into_iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();

        // Insert search bar at top of visible area when search is active
        if state.search.active {
            let match_info = format!(
                "  {}/{}",
                if state.search.match_count > 0 {
                    state.search.current_match + 1
                } else {
                    0
                },
                state.search.match_count
            );
            let search_line = Line::from(vec![
                Span::styled(" / ", self.theme.accent_style()),
                Span::styled(state.search.query.clone(), self.theme.bold_style()),
                Span::styled(match_info, self.theme.dim_style()),
            ]);
            visible.insert(0, search_line);
            // Remove the last line to keep the total count consistent
            if visible.len() > visible_height {
                visible.pop();
            }
        }

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
