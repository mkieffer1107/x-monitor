use chrono::Local;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{AddMonitorForm, App, FocusPane, MonitorFormMode, TargetFilePicker},
    models::{FeedKind, MonitorKind},
};

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(frame.area());

    render_header(frame, app, root[0]);
    render_body(frame, app, root[1]);
    render_details(frame, app, root[2]);
    render_footer(frame, app, root[3]);

    if let Some(form) = &app.add_form {
        render_add_modal(frame, app, form);
    }
    if let Some(picker) = &app.target_file_picker {
        render_target_file_picker(frame, picker);
    }
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let stream_connected = app.stream_connected();
    let now = Local::now().format("%H:%M:%S").to_string();
    let stream_status_text = if stream_connected {
        "Stream: connected"
    } else {
        "Stream: disconnected"
    };
    let stream_status_style = if stream_connected {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };

    let title = Line::from(vec![
        Span::styled(
            "𝕏 Monitor",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  "),
        Span::raw(format!("Monitors: {}", app.monitors.len())),
        Span::raw("  |  "),
        Span::styled(stream_status_text, stream_status_style),
        Span::raw("  |  "),
        Span::raw(&app.status),
    ]);

    let block = Block::default()
        .title("Home")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(12)])
        .split(inner);

    let left = Paragraph::new(title);
    frame.render_widget(left, cols[0]);

    let right = Paragraph::new(now)
        .alignment(Alignment::Right)
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(right, cols[1]);
}

fn render_body(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(area);

    render_monitors(frame, app, columns[0]);
    render_feed(frame, app, columns[1]);
}

fn render_monitors(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let items = if app.monitors.is_empty() {
        vec![ListItem::new(Line::from(
            "No monitors yet. Press 'a' to add one.",
        ))]
    } else {
        app.monitors
            .iter()
            .enumerate()
            .map(|(index, monitor)| {
                let active = monitor.enabled && app.monitor_is_active(monitor.id);
                let selected = index == app.selected_monitor;
                let kind = match monitor.kind {
                    MonitorKind::Account => "acct",
                    MonitorKind::Phrase => "phrase",
                };
                let ai = if monitor.analysis.enabled {
                    format!("AI:{}", monitor.analysis.provider)
                } else {
                    "AI:off".to_string()
                };

                let (status, mut status_style) = if !monitor.enabled {
                    ("off", Style::default().fg(Color::Red))
                } else if active {
                    ("active", Style::default().fg(Color::Green))
                } else {
                    ("inactive", Style::default().fg(Color::Red))
                };
                if selected {
                    status_style = status_style.bg(Color::Blue).add_modifier(Modifier::BOLD);
                }

                let info_style = if selected {
                    Style::default().fg(Color::White).bg(Color::Blue)
                } else {
                    Style::default()
                };

                let mut item = ListItem::new(Line::from(vec![
                    Span::styled(format!("● {status}"), status_style),
                    Span::styled(" ", info_style),
                    Span::styled(format!("{} [{}] {}", monitor.label, kind, ai), info_style),
                ]));
                if selected {
                    item = item.style(Style::default().bg(Color::Blue));
                }
                item
            })
            .collect::<Vec<_>>()
    };

    let mut list_state = ListState::default();
    if !app.monitors.is_empty() {
        list_state.select(Some(app.selected_monitor));
    }

    let title = if app.focus == FocusPane::Monitors {
        "Monitored Targets (focused)"
    } else {
        "Monitored Targets"
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(if app.focus == FocusPane::Monitors {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                }),
        )
        .highlight_symbol("» ");

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let hints = if app.target_file_picker.is_some() {
        Line::from(vec![
            Span::styled("Up/Down", Style::default().fg(Color::Green)),
            Span::raw(" choose file  "),
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" connect from file  "),
            Span::styled("q", Style::default().fg(Color::Green)),
            Span::raw(" close picker"),
        ])
    } else if app.add_form.is_some() {
        Line::from(vec![
            Span::styled("Up/Down", Style::default().fg(Color::Green)),
            Span::raw(" field  "),
            Span::styled("Left/Right", Style::default().fg(Color::Green)),
            Span::raw(" toggle/cycle  "),
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" next/submit  "),
            Span::styled("y", Style::default().fg(Color::Green)),
            Span::raw(" yaml file picker  "),
            Span::styled("q", Style::default().fg(Color::Green)),
            Span::raw(" cancel"),
        ])
    } else {
        Line::from(vec![
            Span::styled("a", Style::default().fg(Color::Green)),
            Span::raw(" add  "),
            Span::styled("e", Style::default().fg(Color::Green)),
            Span::raw(" edit  "),
            Span::styled("d", Style::default().fg(Color::Green)),
            Span::raw(" delete  "),
            Span::styled("s", Style::default().fg(Color::Green)),
            Span::raw(" toggle active  "),
            Span::styled("r", Style::default().fg(Color::Green)),
            Span::raw(" reconnect target  "),
            Span::styled("x", Style::default().fg(Color::Green)),
            Span::raw(" kill conns  "),
            Span::styled("Tab", Style::default().fg(Color::Green)),
            Span::raw(" switch pane  "),
            Span::styled("Up/Down", Style::default().fg(Color::Green)),
            Span::raw(" navigate  "),
            Span::styled("o", Style::default().fg(Color::Green)),
            Span::raw(" open URL  "),
            Span::styled("c", Style::default().fg(Color::Green)),
            Span::raw(" clear feed  "),
            Span::styled("q", Style::default().fg(Color::Green)),
            Span::raw(" quit"),
        ])
    };

    let footer = Paragraph::new(hints).block(
        Block::default()
            .title("Keyboard")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(footer, area);
}

fn render_feed(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let wrap_width = area.width.saturating_sub(4) as usize;
    let items = if app.feed.is_empty() {
        let message = if app.monitors.is_empty() {
            "Add a target to activate live feed."
        } else {
            "Waiting for matching posts..."
        };
        vec![ListItem::new(Line::from(message))]
    } else {
        app.feed
            .iter()
            .map(|event| {
                let style = match event.kind {
                    FeedKind::Post { .. } => Style::default().fg(Color::White),
                    FeedKind::Analysis { .. } => Style::default().fg(Color::LightBlue),
                    FeedKind::Info(_) => Style::default().fg(Color::Gray),
                    FeedKind::Error(_) => Style::default().fg(Color::LightRed),
                };

                let wrapped = wrap_for_width(&event.summary(), wrap_width)
                    .into_iter()
                    .map(|segment| Line::styled(segment, style))
                    .collect::<Vec<_>>();
                ListItem::new(wrapped)
            })
            .collect::<Vec<_>>()
    };

    let mut list_state = ListState::default();
    if !app.feed.is_empty() {
        list_state.select(Some(app.selected_feed));
    }

    let title = if app.focus == FocusPane::Feed {
        "Live Feed (focused)"
    } else {
        "Live Feed"
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(if app.focus == FocusPane::Feed {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                }),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("» ");

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn wrap_for_width(input: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![input.to_string()];
    }

    let mut lines = Vec::new();
    for raw_line in input.lines() {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                if word.chars().count() <= width {
                    current.push_str(word);
                } else {
                    push_split_word(word, width, &mut lines);
                }
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current);
                current = String::new();
                if word.chars().count() <= width {
                    current.push_str(word);
                } else {
                    push_split_word(word, width, &mut lines);
                }
            }
        }

        if !current.is_empty() {
            lines.push(current);
        } else if raw_line.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn push_split_word(word: &str, width: usize, lines: &mut Vec<String>) {
    let mut chunk = String::new();
    for ch in word.chars() {
        if chunk.chars().count() == width {
            lines.push(chunk);
            chunk = String::new();
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        lines.push(chunk);
    }
}

fn render_details(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let text = if app.focus == FocusPane::Monitors {
        if let Some(monitor) = app.selected_monitor() {
            let active = monitor.enabled && app.monitor_is_active(monitor.id);
            let (status_text, status_style) = if !monitor.enabled {
                ("off", Style::default().fg(Color::Red))
            } else if active {
                ("active", Style::default().fg(Color::Green))
            } else {
                ("inactive", Style::default().fg(Color::Red))
            };
            let enabled_style = if monitor.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            vec![
                // Line::from(format!("Display name: {}", monitor.label)),
                Line::from(vec![
                    Span::raw("Enabled: "),
                    Span::styled(if monitor.enabled { "yes" } else { "no" }, enabled_style),
                ]),
                Line::from(vec![
                    Span::raw("Status: "),
                    Span::styled(status_text, status_style),
                ]),
                Line::from(format!("Kind: {}", monitor.kind.display())),
                Line::from(format!("Target: {}", monitor.input_value)),
                Line::from(format!("Query: {}", monitor.query)),
                // Line::from(format!(
                //     "AI: {}",
                //     if monitor.analysis.enabled {
                //         format!("enabled ({})", monitor.analysis.provider)
                //     } else {
                //         "disabled".to_string()
                //     }
                // )),
                Line::from(format!("Prompt: {}", monitor.analysis.prompt)),
                Line::from(format!(
                    "Model: {}",
                    if monitor.analysis.model.trim().is_empty() {
                        "(provider default)".to_string()
                    } else {
                        monitor.analysis.model.clone()
                    }
                )),
                Line::from(format!(
                    "Endpoint: {}",
                    if monitor.analysis.endpoint.trim().is_empty() {
                        "(provider default)".to_string()
                    } else {
                        monitor.analysis.endpoint.clone()
                    }
                )),
                Line::from(format!(
                    "API key: {}",
                    if monitor.analysis.api_key.trim().is_empty() {
                        "(provider default/env)".to_string()
                    } else if is_env_var_name(monitor.analysis.api_key.trim())
                        || monitor
                            .analysis
                            .api_key
                            .trim()
                            .strip_prefix('$')
                            .is_some_and(is_env_var_name)
                    {
                        format!("env ref ({})", monitor.analysis.api_key.trim())
                    } else {
                        "(monitor override)".to_string()
                    }
                )),
            ]
        } else {
            vec![Line::from("Select a monitor to inspect details.")]
        }
    } else if let Some(feed) = app.selected_feed_item() {
        let mut lines = vec![Line::from(feed.summary())];
        if let Some(url) = &feed.url {
            lines.push(Line::from(format!("URL: {url}")));
        }
        lines
    } else {
        vec![Line::from("Select a feed item to inspect details.")]
    };

    let block = Paragraph::new(text).wrap(Wrap { trim: false }).block(
        Block::default()
            .title("Details")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(block, area);
}

fn render_add_modal(frame: &mut Frame<'_>, app: &App, form: &AddMonitorForm) {
    let area = centered_rect(70, 70, frame.area());
    frame.render_widget(Clear, area);
    let blink_on = slow_blink_on();

    let mut lines = Vec::new();
    lines.push(field_line(
        form.field_index == 0,
        format!("Type: {}", form.kind.display()),
        FieldControl::Toggle,
    ));
    lines.push(field_line(
        form.field_index == 1,
        format!(
            "Target: {}",
            with_blink_cursor(&form.target, form.field_index == 1, blink_on)
        ),
        FieldControl::Text,
    ));
    if form.mode == MonitorFormMode::Add {
        lines.push(Line::styled(
            "  YAML target: press 'y' to browse monitor-config files",
            Style::default().fg(Color::DarkGray),
        ));
    }
    if form.kind == MonitorKind::Account {
        lines.push(Line::styled(
            "  handles: comma-separated, '@' optional",
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(field_line(
        form.field_index == 2,
        format!(
            "Display name: {}",
            with_blink_cursor(&form.display_name, form.field_index == 2, blink_on)
        ),
        FieldControl::Text,
    ));
    lines.push(field_line(
        form.field_index == 3,
        format!(
            "Run AI analysis: {}",
            if form.ai_enabled { "Yes" } else { "No" }
        ),
        FieldControl::Toggle,
    ));

    let provider = form.selected_provider(&app.provider_names);
    lines.push(field_line(
        form.field_index == 4,
        format!("AI provider: {provider}"),
        FieldControl::Toggle,
    ));
    lines.push(field_line(
        form.field_index == 5,
        format!(
            "AI model ID: {}",
            with_blink_cursor(&form.ai_model, form.field_index == 5, blink_on)
        ),
        FieldControl::Text,
    ));
    lines.push(field_line(
        form.field_index == 6,
        format!(
            "AI endpoint: {}",
            with_blink_cursor(&form.ai_endpoint, form.field_index == 6, blink_on)
        ),
        FieldControl::Text,
    ));
    lines.push(field_line(
        form.field_index == 7,
        format!(
            "AI API key: {}",
            with_blink_cursor(
                &api_key_input_display(&form.ai_api_key),
                form.field_index == 7,
                blink_on
            )
        ),
        FieldControl::Text,
    ));
    lines.push(field_line(
        form.field_index == 8,
        format!(
            "AI prompt: {}",
            with_blink_cursor(&form.ai_prompt, form.field_index == 8, blink_on)
        ),
        FieldControl::Text,
    ));
    lines.push(field_line(
        form.field_index == 9,
        match form.mode {
            MonitorFormMode::Add => "Create monitor (press Enter)".to_string(),
            MonitorFormMode::Edit => "Save target changes (press Enter)".to_string(),
        },
        FieldControl::Submit,
    ));

    let title = match form.mode {
        MonitorFormMode::Add => "Add Monitor",
        MonitorFormMode::Edit => "Edit Monitor",
    };

    let modal = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(modal, area);
}

fn render_target_file_picker(frame: &mut Frame<'_>, picker: &TargetFilePicker) {
    let area = centered_rect(90, 80, frame.area());
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .title(format!(
            "YAML Target Files ({})",
            picker.directory.display()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(2)])
        .split(inner);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(rows[0]);

    let items = if picker.entries.is_empty() {
        vec![ListItem::new(Line::from(
            "No .yaml/.yml files found in this directory.",
        ))]
    } else {
        picker
            .entries
            .iter()
            .map(|entry| {
                let (status, style) = match &entry.parsed {
                    Ok(_) => ("●", Style::default().fg(Color::Green)),
                    Err(_) => ("●", Style::default().fg(Color::Red)),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(status, style),
                    Span::raw(" "),
                    Span::raw(&entry.file_name),
                ]))
            })
            .collect::<Vec<_>>()
    };

    let mut list_state = ListState::default();
    if !picker.entries.is_empty() {
        list_state.select(Some(
            picker.selected.min(picker.entries.len().saturating_sub(1)),
        ));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .title("Files")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("» ");
    frame.render_stateful_widget(list, cols[0], &mut list_state);

    let preview_lines = picker
        .entries
        .get(picker.selected)
        .map(preview_target_file)
        .unwrap_or_else(|| vec![Line::from("Select a YAML file from the left list.")]);

    let preview = Paragraph::new(preview_lines)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title("Preview")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(preview, cols[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("Up/Down", Style::default().fg(Color::Green)),
        Span::raw(" choose file  "),
        Span::styled("Enter", Style::default().fg(Color::Green)),
        Span::raw(" connect from selected file  "),
        Span::styled("q", Style::default().fg(Color::Green)),
        Span::raw(" close"),
    ]));
    frame.render_widget(hints, rows[1]);
}

fn preview_target_file(entry: &crate::target_files::TargetFileEntry) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(format!("File: {}", entry.file_name)));
    lines.push(Line::from(format!("Path: {}", entry.path.display())));

    match &entry.parsed {
        Ok(target) => {
            lines.push(Line::from(vec![
                Span::raw("Status: "),
                Span::styled("valid", Style::default().fg(Color::Green)),
            ]));
            lines.push(Line::from(format!("Kind: {}", target.kind.display())));
            lines.push(Line::from(format!("Target: {}", target.target)));
            lines.push(Line::from(format!(
                "Display name: {}",
                target.label.clone().unwrap_or_else(|| "(auto)".to_string())
            )));
            lines.push(Line::from(format!(
                "AI: {}",
                if target.ai_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )));
            if let Some(provider) = &target.ai_provider {
                lines.push(Line::from(format!("AI provider: {provider}")));
            }
            if let Some(model) = &target.ai_model {
                lines.push(Line::from(format!("AI model: {model}")));
            }
        }
        Err(error) => {
            lines.push(Line::from(vec![
                Span::raw("Status: "),
                Span::styled("invalid", Style::default().fg(Color::Red)),
            ]));
            lines.push(Line::from(format!("Error: {error}")));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::styled(
        "YAML contents:",
        Style::default().fg(Color::Gray),
    ));

    let raw = if entry.raw.trim().is_empty() {
        "(empty file)".to_string()
    } else {
        entry.raw.clone()
    };
    for raw_line in raw.lines() {
        lines.push(highlight_yaml_line(raw_line));
    }
    if raw.ends_with('\n') {
        lines.push(Line::from(""));
    }

    lines
}

fn highlight_yaml_line(raw_line: &str) -> Line<'static> {
    if raw_line.is_empty() {
        return Line::from(String::new());
    }

    let indent_len = raw_line
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(raw_line.len());
    let indent = &raw_line[..indent_len];
    let body = &raw_line[indent_len..];
    let (content, comment) = split_yaml_comment(body);

    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(Span::styled(
            indent.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.extend(highlight_yaml_content(content));
    if let Some(comment) = comment {
        spans.push(Span::styled(
            comment.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if spans.is_empty() {
        Line::from(raw_line.to_string())
    } else {
        Line::from(spans)
    }
}

fn split_yaml_comment(body: &str) -> (&str, Option<&str>) {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in body.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return (&body[..idx], Some(&body[idx..])),
            _ => {}
        }
    }
    (body, None)
}

fn highlight_yaml_content(content: &str) -> Vec<Span<'static>> {
    if content.is_empty() {
        return vec![];
    }

    let mut spans = Vec::new();
    let leading_ws_len = content
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(content.len());
    let leading_ws = &content[..leading_ws_len];
    let rest = &content[leading_ws_len..];

    if !leading_ws.is_empty() {
        spans.push(Span::styled(
            leading_ws.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }

    if rest.starts_with("- ") {
        spans.push(Span::styled(
            "-".to_string(),
            Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.extend(highlight_yaml_mapping_or_scalar(&rest[2..]));
        return spans;
    }

    spans.extend(highlight_yaml_mapping_or_scalar(rest));
    spans
}

fn highlight_yaml_mapping_or_scalar(text: &str) -> Vec<Span<'static>> {
    if let Some(colon_idx) = find_unquoted_colon(text) {
        let key = &text[..colon_idx];
        let tail = &text[colon_idx + 1..];
        let mut spans = vec![
            Span::styled(
                key.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":".to_string(), Style::default().fg(Color::DarkGray)),
        ];

        let tail_ws_len = tail
            .find(|ch: char| !ch.is_whitespace())
            .unwrap_or(tail.len());
        let tail_ws = &tail[..tail_ws_len];
        let value = &tail[tail_ws_len..];

        if !tail_ws.is_empty() {
            spans.push(Span::raw(tail_ws.to_string()));
        }
        if !value.is_empty() {
            spans.push(Span::styled(value.to_string(), yaml_value_style(value)));
        }

        spans
    } else {
        vec![Span::styled(text.to_string(), yaml_value_style(text))]
    }
}

fn find_unquoted_colon(input: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in input.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => return Some(idx),
            _ => {}
        }
    }
    None
}

fn yaml_value_style(value: &str) -> Style {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Style::default().fg(Color::White);
    }

    let bool_like = matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off"
    );
    if bool_like {
        return Style::default().fg(Color::Green);
    }

    let null_like = matches!(trimmed.to_ascii_lowercase().as_str(), "null" | "~");
    if null_like {
        return Style::default().fg(Color::Gray);
    }

    if trimmed.parse::<f64>().is_ok() {
        return Style::default().fg(Color::LightMagenta);
    }

    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return Style::default().fg(Color::Yellow);
    }

    if trimmed.starts_with('&') || trimmed.starts_with('*') {
        return Style::default().fg(Color::LightCyan);
    }

    Style::default().fg(Color::White)
}

#[derive(Debug, Clone, Copy)]
enum FieldControl {
    Toggle,
    Text,
    Submit,
}

fn field_line(selected: bool, text: String, control: FieldControl) -> Line<'static> {
    if selected {
        let mut spans = vec![
            Span::styled(
                "> ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::default().fg(Color::Yellow)),
        ];

        let hint = match control {
            FieldControl::Toggle => "[<- ->]",
            FieldControl::Text => "[type]",
            FieldControl::Submit => "[Enter]",
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(hint, Style::default().fg(Color::Green)));

        Line::from(spans)
    } else {
        Line::from(format!("  {text}"))
    }
}

fn with_blink_cursor(input: &str, active: bool, blink_on: bool) -> String {
    if !active {
        return input.to_string();
    }
    let cursor = if blink_on { "_" } else { " " };
    format!("{input}{cursor}")
}

fn mask_secret(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    "*".repeat(input.chars().count())
}

fn api_key_input_display(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if let Some(var_name) = trimmed.strip_prefix('$') {
        if is_env_var_name(var_name) {
            return trimmed.to_string();
        }
    } else if is_env_var_name(trimmed) {
        return trimmed.to_string();
    }

    mask_secret(trimmed)
}

fn is_env_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

fn slow_blink_on() -> bool {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    (millis / 700) % 2 == 0
}

fn centered_rect(percent_x: u16, percent_y: u16, rect: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(rect);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
        .inner(Margin {
            vertical: 0,
            horizontal: 0,
        })
}
