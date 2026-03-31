//! Thread list sidebar panel.
//!
//! Renders a compact status table of active threads/jobs:
//!
//! ```text
//! THREADS (3) ─────────
//! ● main        active  2m
//! ○ background  idle    15m
//! ✓ job-123     done    5m
//! ✗ job-456     failed  1m
//! ```

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::layout::TuiSlot;
use crate::render::truncate;
use crate::theme::Theme;

use super::{AppState, ThreadStatus, TuiWidget};

/// Status icon for each thread state.
fn status_icon(status: ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Active => "\u{25CF}",    // ●
        ThreadStatus::Idle => "\u{25CB}",      // ○
        ThreadStatus::Completed => "\u{2713}", // ✓
        ThreadStatus::Failed => "\u{2717}",    // ✗
    }
}

/// Format a duration in seconds into a compact human-readable string.
fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{h}h")
        }
    }
}

pub struct ThreadListWidget {
    theme: Theme,
}

impl ThreadListWidget {
    pub fn new(theme: Theme) -> Self {
        Self { theme }
    }

    /// Pick the style for a thread's status icon and text.
    fn status_style(&self, status: ThreadStatus) -> ratatui::style::Style {
        match status {
            ThreadStatus::Active => self.theme.accent_style(),
            ThreadStatus::Idle => self.theme.dim_style(),
            ThreadStatus::Completed => self.theme.success_style(),
            ThreadStatus::Failed => self.theme.error_style(),
        }
    }
}

impl TuiWidget for ThreadListWidget {
    fn id(&self) -> &str {
        "thread_list"
    }

    fn slot(&self) -> TuiSlot {
        TuiSlot::SidebarSection
    }

    fn render(&self, area: Rect, buf: &mut Buffer, state: &AppState) {
        if area.height == 0 || area.width < 4 {
            return;
        }

        let width = area.width as usize;
        let mut lines: Vec<Line<'_>> = Vec::new();

        // ── Header line with thread count and horizontal rule ──────────
        let header_text = format!(" THREADS ({})", state.threads.len());
        let rule_len = width.saturating_sub(header_text.len() + 1);
        let rule = if rule_len > 0 {
            format!(" {}", "\u{2500}".repeat(rule_len))
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            Span::styled(header_text, self.theme.bold_style()),
            Span::styled(rule, self.theme.dim_style()),
        ]));

        if state.threads.is_empty() {
            lines.push(Line::from(Span::styled(
                " (no threads)",
                self.theme.dim_style(),
            )));
        }

        // ── Compute column widths ─────────────────────────────────────
        // Layout: " {icon} {name}  {status}  {uptime}"
        // Icon column: 3 chars (" X ")
        // Status text: max 6 chars ("active")
        // Uptime: max ~8 chars ("99h 59m")
        // Spacing: 2 + 2 = 4 padding chars
        // Name gets whatever is left
        let fixed_cols: usize = 3 + 6 + 8 + 4; // icon + status + uptime + spacing
        let max_name_len = width.saturating_sub(fixed_cols).max(4);

        let now = chrono::Utc::now();

        for thread in &state.threads {
            let style = self.status_style(thread.status);
            let icon = status_icon(thread.status);

            // Compute uptime from started_at
            let uptime_secs = now
                .signed_duration_since(thread.started_at)
                .num_seconds()
                .max(0) as u64;
            let uptime = format_uptime(uptime_secs);

            // Truncate name to fit available width
            let name = if thread.label.is_empty() {
                truncate(&thread.id, max_name_len)
            } else {
                truncate(&thread.label, max_name_len)
            };

            // Right-pad name to align status column
            let padded_name = format!("{:<width$}", name, width = max_name_len);

            let status_text = format!("{}", thread.status);

            lines.push(Line::from(vec![
                Span::styled(format!(" {icon} "), style),
                Span::styled(padded_name, self.theme.bold_style()),
                Span::raw("  "),
                Span::styled(format!("{:<6}", status_text), style),
                Span::raw("  "),
                Span::styled(uptime, self.theme.dim_style()),
            ]));
        }

        let visible: Vec<Line<'_>> = lines.into_iter().take(area.height as usize).collect();
        let paragraph = ratatui::widgets::Paragraph::new(visible);
        paragraph.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use crate::widgets::{AppState, ThreadInfo, ThreadStatus};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn make_state_with_threads(threads: Vec<ThreadInfo>) -> AppState {
        let mut state = AppState::default();
        state.threads = threads;
        state
    }

    fn render_to_buffer(widget: &ThreadListWidget, state: &AppState, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf, state);
        buf
    }

    fn buffer_text(buf: &Buffer) -> String {
        let area = buf.area;
        let mut text = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn empty_threads_shows_no_threads() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![]);
        let buf = render_to_buffer(&widget, &state, 40, 5);
        let text = buffer_text(&buf);
        assert!(text.contains("THREADS (0)"));
        assert!(text.contains("(no threads)"));
    }

    #[test]
    fn renders_active_thread() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![ThreadInfo {
            id: "t1".to_string(),
            label: "main".to_string(),
            is_foreground: true,
            is_running: true,
            duration_secs: 120,
            status: ThreadStatus::Active,
            started_at: chrono::Utc::now() - chrono::Duration::seconds(120),
        }]);
        let buf = render_to_buffer(&widget, &state, 50, 5);
        let text = buffer_text(&buf);
        assert!(text.contains("THREADS (1)"));
        assert!(text.contains("main"));
        assert!(text.contains("active"));
        assert!(text.contains("\u{25CF}")); // ● icon
    }

    #[test]
    fn renders_idle_thread() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![ThreadInfo {
            id: "t2".to_string(),
            label: "background".to_string(),
            is_foreground: false,
            is_running: true,
            duration_secs: 900,
            status: ThreadStatus::Idle,
            started_at: chrono::Utc::now() - chrono::Duration::seconds(900),
        }]);
        let buf = render_to_buffer(&widget, &state, 50, 5);
        let text = buffer_text(&buf);
        assert!(text.contains("idle"));
        assert!(text.contains("\u{25CB}")); // ○ icon
    }

    #[test]
    fn renders_completed_thread() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![ThreadInfo {
            id: "job-123".to_string(),
            label: "job-123".to_string(),
            is_foreground: false,
            is_running: false,
            duration_secs: 300,
            status: ThreadStatus::Completed,
            started_at: chrono::Utc::now() - chrono::Duration::seconds(300),
        }]);
        let buf = render_to_buffer(&widget, &state, 50, 5);
        let text = buffer_text(&buf);
        assert!(text.contains("done"));
        assert!(text.contains("\u{2713}")); // ✓ icon
    }

    #[test]
    fn renders_failed_thread() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![ThreadInfo {
            id: "job-456".to_string(),
            label: "job-456".to_string(),
            is_foreground: false,
            is_running: false,
            duration_secs: 60,
            status: ThreadStatus::Failed,
            started_at: chrono::Utc::now() - chrono::Duration::seconds(60),
        }]);
        let buf = render_to_buffer(&widget, &state, 50, 5);
        let text = buffer_text(&buf);
        assert!(text.contains("failed"));
        assert!(text.contains("\u{2717}")); // ✗ icon
    }

    #[test]
    fn multiple_threads_render() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let now = chrono::Utc::now();
        let state = make_state_with_threads(vec![
            ThreadInfo {
                id: "t1".to_string(),
                label: "main".to_string(),
                is_foreground: true,
                is_running: true,
                duration_secs: 120,
                status: ThreadStatus::Active,
                started_at: now - chrono::Duration::seconds(120),
            },
            ThreadInfo {
                id: "t2".to_string(),
                label: "worker".to_string(),
                is_foreground: false,
                is_running: true,
                duration_secs: 900,
                status: ThreadStatus::Idle,
                started_at: now - chrono::Duration::seconds(900),
            },
            ThreadInfo {
                id: "t3".to_string(),
                label: "cleanup".to_string(),
                is_foreground: false,
                is_running: false,
                duration_secs: 45,
                status: ThreadStatus::Completed,
                started_at: now - chrono::Duration::seconds(45),
            },
        ]);
        let buf = render_to_buffer(&widget, &state, 50, 8);
        let text = buffer_text(&buf);
        assert!(text.contains("THREADS (3)"));
        assert!(text.contains("main"));
        assert!(text.contains("worker"));
        assert!(text.contains("cleanup"));
    }

    #[test]
    fn too_small_area_renders_nothing() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![]);
        // Width < 4 should bail out
        let buf = render_to_buffer(&widget, &state, 3, 5);
        let text = buffer_text(&buf);
        // Buffer should be mostly blank
        assert!(!text.contains("THREADS"));
    }

    #[test]
    fn zero_height_renders_nothing() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![ThreadInfo {
            id: "t1".to_string(),
            label: "main".to_string(),
            is_foreground: true,
            is_running: true,
            duration_secs: 10,
            status: ThreadStatus::Active,
            started_at: chrono::Utc::now(),
        }]);
        let buf = render_to_buffer(&widget, &state, 40, 0);
        let text = buffer_text(&buf);
        assert!(text.is_empty() || text.trim().is_empty());
    }

    #[test]
    fn thread_status_display() {
        assert_eq!(format!("{}", ThreadStatus::Active), "active");
        assert_eq!(format!("{}", ThreadStatus::Idle), "idle");
        assert_eq!(format!("{}", ThreadStatus::Completed), "done");
        assert_eq!(format!("{}", ThreadStatus::Failed), "failed");
    }

    #[test]
    fn format_uptime_seconds() {
        assert_eq!(super::format_uptime(30), "30s");
    }

    #[test]
    fn format_uptime_minutes() {
        assert_eq!(super::format_uptime(150), "2m");
    }

    #[test]
    fn format_uptime_hours() {
        assert_eq!(super::format_uptime(3720), "1h 2m");
    }

    #[test]
    fn format_uptime_exact_hour() {
        assert_eq!(super::format_uptime(7200), "2h");
    }

    #[test]
    fn label_falls_back_to_id() {
        let theme = Theme::dark();
        let widget = ThreadListWidget::new(theme);
        let state = make_state_with_threads(vec![ThreadInfo {
            id: "abc-def".to_string(),
            label: String::new(),
            is_foreground: false,
            is_running: true,
            duration_secs: 10,
            status: ThreadStatus::Active,
            started_at: chrono::Utc::now(),
        }]);
        let buf = render_to_buffer(&widget, &state, 50, 5);
        let text = buffer_text(&buf);
        assert!(text.contains("abc-def"));
    }
}
