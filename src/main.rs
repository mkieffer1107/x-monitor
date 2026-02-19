mod ai;
mod app;
mod config;
mod models;
mod target_files;
mod ui;
mod x_api;

use std::{
    fs::{self, File},
    io::{self, BufWriter, Stdout, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use ai::AiClient;
use anyhow::{Context, Result};
use app::{App, PendingMonitor};
use chrono::{Local, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dotenvy::dotenv;
use models::{Monitor, StreamPost};
use ratatui::{Terminal, backend::CrosstermBackend};
use target_files::TargetFileMonitor;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use x_api::XApiClient;

#[derive(Debug, Clone)]
enum AppMsg {
    Info(String),
    Error(String),
    StreamConnectionState(bool),
    StreamPost(StreamPost),
    MonitorAdded(Result<Monitor, String>),
    MonitorEditPrepared {
        monitor_id: Uuid,
        result: Result<Monitor, String>,
    },
    MonitorEdited(Result<Monitor, String>),
    MonitorActivated(Result<(Uuid, String, String), String>),
    MonitorDeactivated(Result<(Uuid, String), String>),
    MonitorDeleted(Result<(Uuid, String), String>),
    MonitorReconnected(Result<(Uuid, String, String), String>),
    AnalysisCompleted {
        monitor_label: String,
        provider: String,
        model: String,
        output: Result<String, String>,
        url: Option<String>,
    },
}

#[derive(Debug, Default)]
struct CliArgs {
    log_session: bool,
    log_file: Option<PathBuf>,
}

#[derive(Debug)]
struct SessionLogger {
    path: PathBuf,
    writer: BufWriter<File>,
    last_feed_id: Option<Uuid>,
}

impl SessionLogger {
    fn from_cli(cli: &CliArgs) -> Result<Option<Self>> {
        if cli.log_session && cli.log_file.is_some() {
            anyhow::bail!("use either --log-session or --log-file <path>, not both");
        }

        let path = if let Some(path) = &cli.log_file {
            Some(resolve_log_path(path)?)
        } else if cli.log_session {
            Some(default_session_log_path()?)
        } else {
            None
        };

        path.map(Self::new).transpose()
    }

    fn new(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create log directory {}", parent.display())
                })?;
            }
        }

        let file =
            File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
            last_feed_id: None,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn log_line(&mut self, line: &str) -> Result<()> {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S");
        writeln!(self.writer, "[{now}] {line}").context("failed to write to session log")?;
        self.writer.flush().context("failed to flush session log")
    }

    fn flush_new_feed_items(&mut self, app: &App) -> Result<()> {
        let ordered = app.feed.iter().rev().collect::<Vec<_>>();

        let start_index = if let Some(last_id) = self.last_feed_id {
            ordered
                .iter()
                .position(|item| item.id == last_id)
                .map(|index| index + 1)
                .unwrap_or(0)
        } else {
            0
        };

        for item in ordered.iter().skip(start_index) {
            let mut line = item.summary();
            if let Some(url) = &item.url {
                line.push_str(&format!(" | URL: {url}"));
            }
            self.log_line(&line)?;
        }

        self.last_feed_id = ordered.last().map(|item| item.id);
        Ok(())
    }
}

fn parse_cli_args() -> Result<CliArgs> {
    let mut cli = CliArgs::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log-session" => cli.log_session = true,
            "--log-file" => {
                let value = args.next().context("--log-file requires a path argument")?;
                cli.log_file = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {
                anyhow::bail!("unknown argument '{arg}'. Run with --help.");
            }
        }
    }

    Ok(cli)
}

fn print_usage() {
    println!("x-monitor");
    println!("Usage:");
    println!("  cargo run -- [--log-session | --log-file <path>]");
    println!();
    println!("Options:");
    println!("  --log-session      Write log to ./logs/session-YYYYMMDD-HHMMSS.log");
    println!("  --log-file <path>  Write log to a custom file path");
    println!("  -h, --help         Show this help");
}

fn resolve_log_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn default_session_log_path() -> Result<PathBuf> {
    let filename = format!("session-{}.log", Local::now().format("%Y%m%d-%H%M%S"));
    let relative = PathBuf::from("logs").join(filename);
    resolve_log_path(&relative)
}

fn flush_session_logs(app: &mut App, session_logger: &mut Option<SessionLogger>) {
    let Some(logger) = session_logger.as_mut() else {
        return;
    };

    if let Err(error) = logger.flush_new_feed_items(app) {
        let message = format!("session logging disabled: {error}");
        *session_logger = None;
        app.push_error(message);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    let cli = parse_cli_args()?;
    let mut session_logger = SessionLogger::from_cli(&cli)?;

    let (mut config, config_path, created_default_config) = config::AppConfig::load()?;
    let config_dir_result = prepare_monitor_config_dir(&mut config);

    let state_path = if config.state_path.is_relative() {
        std::env::current_dir()?.join(&config.state_path)
    } else {
        config.state_path.clone()
    };

    let monitors = App::load_store(&state_path).unwrap_or_else(|error| {
        eprintln!("failed to load state: {error}");
        Vec::new()
    });

    let mut app = App::new(config.clone(), state_path, monitors);

    if let Err(error) = config_dir_result {
        app.push_error(format!(
            "failed to prepare monitor config directory: {error}"
        ));
    }

    if let Some(logger) = session_logger.as_mut() {
        app.push_info(format!("session logging to {}", logger.path().display()));
        let _ = logger.log_line("session started");
    }

    if created_default_config {
        app.push_info(format!(
            "created default config at {} (set 𝕏 bearer token before streaming)",
            config_path.display()
        ));
    }

    if config.x_bearer_token.is_none() {
        app.push_error(
            "𝕏 bearer token is missing. Set X_BEARER_TOKEN (or x_bearer_token) in your environment.",
        );
    }

    let ai_client = AiClient::new()?;
    let x_client = config
        .x_bearer_token
        .clone()
        .map(XApiClient::new)
        .transpose()?;

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<AppMsg>();

    let mut terminal = setup_terminal().context("failed to initialize terminal")?;

    let run_result = run_app(
        &mut terminal,
        &mut app,
        msg_tx.clone(),
        &mut msg_rx,
        x_client,
        ai_client,
        &mut session_logger,
    )
    .await;

    if let Some(logger) = session_logger.as_mut() {
        if let Err(error) = &run_result {
            let _ = logger.log_line(&format!("application error: {error:?}"));
        }
        let _ = logger.log_line("session ended");
    }

    restore_terminal(&mut terminal).ok();

    if let Err(error) = run_result {
        eprintln!("application error: {error:?}");
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    msg_rx: &mut mpsc::UnboundedReceiver<AppMsg>,
    x_client: Option<XApiClient>,
    ai_client: AiClient,
    session_logger: &mut Option<SessionLogger>,
) -> Result<()> {
    let mut stream_shutdown_tx: Option<watch::Sender<bool>> = None;
    reconcile_stream_connection(app, &x_client, &msg_tx, &mut stream_shutdown_tx);
    flush_session_logs(app, session_logger);

    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        while let Ok(message) = msg_rx.try_recv() {
            handle_message(app, message, msg_tx.clone(), ai_client.clone());
        }
        reconcile_stream_connection(app, &x_client, &msg_tx, &mut stream_shutdown_tx);
        flush_session_logs(app, session_logger);

        if app.should_quit {
            if let Some(shutdown_tx) = stream_shutdown_tx.take() {
                let _ = shutdown_tx.send(true);
            }
            if let Err(error) = app.save_store() {
                app.push_error(format!("failed to persist state: {error}"));
            }
            flush_session_logs(app, session_logger);
            break;
        }

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key_event) = event::read()? {
                if key_event.kind == KeyEventKind::Press {
                    handle_key(app, key_event, msg_tx.clone(), x_client.clone());
                }
            }
        }

        flush_session_logs(app, session_logger);
    }

    Ok(())
}

fn reconcile_stream_connection(
    app: &mut App,
    x_client: &Option<XApiClient>,
    msg_tx: &mpsc::UnboundedSender<AppMsg>,
    stream_shutdown_tx: &mut Option<watch::Sender<bool>>,
) {
    let should_run_stream = x_client.is_some() && app.has_enabled_monitors();

    match (should_run_stream, stream_shutdown_tx.is_some()) {
        (true, false) => {
            let Some(client) = x_client.clone() else {
                return;
            };
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let tx = msg_tx.clone();
            tokio::spawn(client.stream_loop(tx, shutdown_rx));
            *stream_shutdown_tx = Some(shutdown_tx);
            app.push_info("stream started");
        }
        (false, true) => {
            if let Some(shutdown_tx) = stream_shutdown_tx.take() {
                let _ = shutdown_tx.send(true);
            }
            if x_client.is_some() && !app.has_enabled_monitors() {
                app.push_info("stream paused (activate or add a target to reconnect)");
            }
        }
        _ => {}
    }
}

fn handle_message(
    app: &mut App,
    message: AppMsg,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    ai_client: AiClient,
) {
    match message {
        AppMsg::Info(info) => app.push_info(info),
        AppMsg::Error(error) => app.push_error(error),
        AppMsg::StreamConnectionState(connected) => {
            app.set_stream_connected(connected);
        }
        AppMsg::MonitorAdded(result) => match result {
            Ok(monitor) => {
                app.add_monitor(monitor.clone());
                if let Err(error) = app.save_store() {
                    app.push_error(format!("monitor added but state save failed: {error}"));
                }
                app.push_info(format!("monitor added: {}", monitor.label));
            }
            Err(error) => app.push_error(format!("failed to add monitor: {error}")),
        },
        AppMsg::MonitorEditPrepared { monitor_id, result } => match result {
            Ok(monitor) => {
                app.open_edit_form(monitor.clone());
                app.push_info(format!("editing target '{}'", monitor.label));
            }
            Err(error) => {
                app.set_monitor_active(monitor_id, app.stream_connected());
                app.push_error(format!("failed to prepare target edit: {error}"));
            }
        },
        AppMsg::MonitorEdited(result) => match result {
            Ok(updated_monitor) => {
                if app.replace_monitor(updated_monitor.clone()) {
                    if let Err(error) = app.save_store() {
                        app.push_error(format!("target updated but state save failed: {error}"));
                    }
                    app.push_info(format!("target updated: {}", updated_monitor.label));
                } else {
                    app.push_error("target update completed but monitor no longer exists");
                }
            }
            Err(error) => app.push_error(format!("failed to update target: {error}")),
        },
        AppMsg::MonitorActivated(result) => match result {
            Ok((monitor_id, label, new_rule_id)) => {
                if app.activate_monitor_with_rule(monitor_id, new_rule_id) {
                    if let Err(error) = app.save_store() {
                        app.push_error(format!("target activated but state save failed: {error}"));
                    }
                    app.push_info(format!("target activated: {label}"));
                }
            }
            Err(error) => app.push_error(format!("failed to activate target: {error}")),
        },
        AppMsg::MonitorDeactivated(result) => match result {
            Ok((monitor_id, label)) => {
                if app.deactivate_monitor(monitor_id) {
                    if let Err(error) = app.save_store() {
                        app.push_error(format!(
                            "target deactivated but state save failed: {error}"
                        ));
                    }
                    app.push_info(format!("target deactivated: {label}"));
                }
            }
            Err(error) => app.push_error(format!("failed to deactivate target: {error}")),
        },
        AppMsg::MonitorDeleted(result) => match result {
            Ok((monitor_id, label)) => {
                if app.remove_monitor_by_id(monitor_id).is_some() {
                    if let Err(error) = app.save_store() {
                        app.push_error(format!("monitor deleted but state save failed: {error}"));
                    }
                    app.push_info(format!("monitor removed: {label}"));
                }
            }
            Err(error) => app.push_error(format!("failed to delete monitor: {error}")),
        },
        AppMsg::MonitorReconnected(result) => match result {
            Ok((monitor_id, label, new_rule_id)) => {
                if app.update_monitor_rule_id(monitor_id, new_rule_id) {
                    if let Err(error) = app.save_store() {
                        app.push_error(format!(
                            "target reconnected but state save failed: {error}"
                        ));
                    }
                    app.push_info(format!("target reconnected: {label}"));
                }
            }
            Err(error) => app.push_error(format!("failed to reconnect target: {error}")),
        },
        AppMsg::StreamPost(post) => {
            let matched = post
                .matching_tags
                .iter()
                .filter_map(|tag| app.monitor_by_tag(tag))
                .filter(|monitor| monitor.enabled)
                .cloned()
                .collect::<Vec<_>>();

            let labels = matched
                .iter()
                .map(|monitor| monitor.label.clone())
                .collect();
            app.push_post(&post, labels);

            for monitor in matched {
                if !monitor.analysis.enabled {
                    continue;
                }

                let provider_name = monitor.analysis.provider.clone();
                let Some(provider_config) = app.config.provider_by_name(&provider_name) else {
                    app.push_error(format!("AI provider '{}' is not configured", provider_name));
                    continue;
                };
                let api_key_override = resolve_api_key_input(&monitor.analysis.api_key);

                let provider = if let Some(resolved) = app.config.resolve_provider(&provider_name) {
                    resolved
                } else if let Some(api_key) = api_key_override.clone() {
                    config::ResolvedAiProvider {
                        name: provider_config.name.clone(),
                        base_url: provider_config.base_url.clone(),
                        model: provider_config.model.clone(),
                        api_key,
                    }
                } else {
                    app.push_error(format!(
                        "AI provider '{}' missing or no API key available",
                        monitor.analysis.provider
                    ));
                    continue;
                };

                let model_id = if monitor.analysis.model.trim().is_empty() {
                    provider.model.clone()
                } else {
                    monitor.analysis.model.trim().to_string()
                };
                if model_id.is_empty() {
                    app.push_error(format!(
                        "analysis skipped for '{}' because model ID is empty",
                        monitor.label
                    ));
                    continue;
                }

                let mut provider = provider;
                if !monitor.analysis.endpoint.trim().is_empty() {
                    provider.base_url = monitor.analysis.endpoint.trim().to_string();
                }
                if let Some(api_key) = api_key_override {
                    provider.api_key = api_key;
                }

                let tx = msg_tx.clone();
                let client = ai_client.clone();
                let prompt = monitor.analysis.prompt.clone();
                let post_text = post.text.clone();
                let monitor_label = monitor.label.clone();
                let provider_name_for_msg = provider.name.clone();
                let model_name = model_id.clone();
                let url = Some(post.post_url());

                tokio::spawn(async move {
                    let output = client
                        .analyze_post(provider, model_id, prompt, post_text)
                        .await
                        .map_err(|error| error.to_string());
                    let _ = tx.send(AppMsg::AnalysisCompleted {
                        monitor_label,
                        provider: provider_name_for_msg,
                        model: model_name,
                        output,
                        url,
                    });
                });
            }
        }
        AppMsg::AnalysisCompleted {
            monitor_label,
            provider,
            model,
            output,
            url,
        } => match output {
            Ok(text) => app.push_analysis(monitor_label, provider, model, text, url),
            Err(error) => app.push_error(format!(
                "analysis failed for '{monitor_label}' via {provider}:{model}: {error}"
            )),
        },
    }
}

fn handle_key(
    app: &mut App,
    key_event: KeyEvent,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    if app.target_file_picker.is_some() {
        handle_target_file_picker_key(app, key_event, msg_tx, x_client);
        return;
    }

    if app.add_form.is_some() {
        handle_add_form_key(app, key_event, msg_tx, x_client);
        return;
    }

    match key_event.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Up => app.move_selection_up(),
        KeyCode::Down => app.move_selection_down(),
        KeyCode::Char('a') => app.open_add_form(),
        KeyCode::Char('e') => edit_selected_monitor(app, msg_tx, x_client),
        KeyCode::Char('s') => toggle_selected_monitor_activation(app, msg_tx, x_client),
        KeyCode::Char('d') => delete_selected_monitor(app, msg_tx, x_client),
        KeyCode::Char('r') => reconnect_selected_monitor(app, msg_tx, x_client),
        KeyCode::Char('x') => terminate_all_connections(app, msg_tx, x_client),
        KeyCode::Char('o') => open_selected_feed_url(app),
        KeyCode::Char('c') => {
            app.clear_feed();
            app.status = "Feed cleared".to_string();
        }
        _ => {}
    }
}

fn handle_add_form_key(
    app: &mut App,
    key_event: KeyEvent,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let active_field = app.add_form.as_ref().map(|form| form.field_index);
    let editing_text = matches!(active_field, Some(1 | 2 | 5 | 6 | 7 | 8));
    let is_editing_monitor = app.edit_session.is_some();

    if key_event.code == KeyCode::Char('q') && !editing_text {
        if is_editing_monitor {
            cancel_monitor_edit(app, msg_tx, x_client);
        } else {
            app.close_add_form();
        }
        return;
    }
    if key_event.code == KeyCode::Esc {
        if is_editing_monitor {
            cancel_monitor_edit(app, msg_tx, x_client);
        } else {
            app.close_add_form();
        }
        return;
    }
    if key_event.code == KeyCode::Char('y') && !is_editing_monitor && !editing_text {
        open_target_file_picker(app);
        return;
    }

    let mut submit_form = false;
    let Some(form) = app.add_form.as_mut() else {
        return;
    };

    match key_event.code {
        KeyCode::Up => form.move_field(-1),
        KeyCode::Down | KeyCode::Tab => form.move_field(1),
        KeyCode::BackTab => form.move_field(-1),
        KeyCode::Left => match form.field_index {
            0 => form.cycle_kind(-1),
            3 => form.ai_enabled = !form.ai_enabled,
            4 => {
                form.cycle_provider(&app.provider_names, -1);
                form.apply_provider_defaults(&app.config, &app.provider_names);
            }
            _ => {}
        },
        KeyCode::Right => match form.field_index {
            0 => form.cycle_kind(1),
            3 => form.ai_enabled = !form.ai_enabled,
            4 => {
                form.cycle_provider(&app.provider_names, 1);
                form.apply_provider_defaults(&app.config, &app.provider_names);
            }
            _ => {}
        },
        KeyCode::Backspace => match form.field_index {
            1 => {
                form.target.pop();
            }
            2 => {
                form.display_name.pop();
            }
            5 => {
                form.ai_model.pop();
            }
            6 => {
                form.ai_endpoint.pop();
            }
            7 => {
                form.ai_api_key.pop();
            }
            8 => {
                form.ai_prompt.pop();
            }
            _ => {}
        },
        KeyCode::Enter => {
            if form.field_index == 9 {
                submit_form = true;
            } else {
                form.move_field(1);
            }
        }
        KeyCode::Char(ch) => {
            if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                return;
            }

            match form.field_index {
                1 => form.target.push(ch),
                2 => form.display_name.push(ch),
                5 => form.ai_model.push(ch),
                6 => form.ai_endpoint.push(ch),
                7 => form.ai_api_key.push(ch),
                8 => form.ai_prompt.push(ch),
                _ => {}
            }
        }
        _ => {}
    }

    if submit_form {
        submit_monitor_form(app, msg_tx, x_client);
    }
}

fn open_target_file_picker(app: &mut App) {
    match app.open_target_file_picker() {
        Ok(0) => app.push_info(format!(
            "no YAML files found in {}",
            app.config.monitor_config_dir.display()
        )),
        Ok(count) => app.push_info(format!(
            "loaded {count} YAML target file{}",
            if count == 1 { "" } else { "s" }
        )),
        Err(error) => app.push_error(format!("failed to load YAML target files: {error}")),
    }
}

fn handle_target_file_picker_key(
    app: &mut App,
    key_event: KeyEvent,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    match key_event.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.close_target_file_picker();
            app.push_info("closed YAML target picker");
        }
        KeyCode::Up => app.move_target_file_selection(-1),
        KeyCode::Down => app.move_target_file_selection(1),
        KeyCode::Enter => select_target_file(app, msg_tx, x_client),
        _ => {}
    }
}

fn select_target_file(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(entry) = app.selected_target_file_entry().cloned() else {
        app.push_info("no YAML target file selected");
        return;
    };

    let target = match entry.parsed {
        Ok(target) => target,
        Err(error) => {
            app.push_error(format!(
                "invalid YAML target file '{}': {error}",
                entry.file_name
            ));
            return;
        }
    };

    app.close_target_file_picker();

    if let Err(error) = apply_target_file_to_form(app, &target) {
        app.push_error(format!(
            "failed to apply YAML target file '{}': {error}",
            entry.file_name
        ));
        return;
    }

    app.push_info(format!(
        "selected YAML target file '{}'; connecting...",
        entry.file_name
    ));
    submit_monitor_form(app, msg_tx, x_client);
}

fn apply_target_file_to_form(app: &mut App, target: &TargetFileMonitor) -> Result<()> {
    let Some(form) = app.add_form.as_mut() else {
        anyhow::bail!("target form is not open");
    };

    form.kind = target.kind.clone();
    form.target = target.target.trim().to_string();
    form.display_name = target.label.clone().unwrap_or_default();

    form.ai_enabled = target.ai_enabled;
    if target.ai_enabled {
        if let Some(provider_name) = &target.ai_provider {
            let Some(index) = app
                .provider_names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(provider_name))
            else {
                anyhow::bail!("AI provider '{}' is not configured", provider_name);
            };
            form.ai_provider_index = index;
        }

        form.apply_provider_defaults(&app.config, &app.provider_names);

        if let Some(model) = &target.ai_model {
            form.ai_model = model.clone();
        }
        if let Some(endpoint) = &target.ai_endpoint {
            form.ai_endpoint = endpoint.clone();
        }
        if let Some(api_key) = &target.ai_api_key {
            form.ai_api_key = api_key.clone();
        }
        if let Some(prompt) = &target.ai_prompt {
            form.ai_prompt = prompt.clone();
        }
    }

    Ok(())
}

fn submit_monitor_form(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    if app.edit_session.is_some() {
        submit_monitor_edit(app, msg_tx, x_client);
        return;
    }

    let Some(form) = app.add_form.clone() else {
        return;
    };

    let Some(client) = x_client else {
        app.push_error("cannot add monitor without 𝕏 bearer token");
        return;
    };

    let pending = match form.to_pending_monitor(&app.provider_names) {
        Ok(pending) => pending,
        Err(error) => {
            app.push_error(format!("invalid monitor settings: {error}"));
            return;
        }
    };

    app.close_add_form();
    app.push_info(format!("adding monitor '{}'...", pending.label));

    tokio::spawn(async move {
        let result = create_monitor(client, pending)
            .await
            .map_err(|error| error.to_string());
        let _ = msg_tx.send(AppMsg::MonitorAdded(result));
    });
}

fn submit_monitor_edit(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(client) = x_client else {
        app.push_error("cannot edit target without 𝕏 bearer token");
        return;
    };

    let Some(form) = app.add_form.clone() else {
        return;
    };
    let Some(session) = app.edit_session.clone() else {
        app.push_error("internal error: edit session missing");
        return;
    };

    let mut pending = match form.to_pending_monitor(&app.provider_names) {
        Ok(pending) => pending,
        Err(error) => {
            app.push_error(format!("invalid monitor settings: {error}"));
            return;
        }
    };
    pending.id = session.original_monitor.id;
    pending.rule_tag = session.original_monitor.rule_tag.clone();

    app.close_add_form();
    app.push_info(format!(
        "saving edits and reconnecting target '{}'...",
        session.original_monitor.label
    ));

    tokio::spawn(async move {
        match create_monitor(client.clone(), pending).await {
            Ok(updated_monitor) => {
                let updated_monitor = Monitor {
                    created_at: session.original_monitor.created_at,
                    ..updated_monitor
                };
                let _ = msg_tx.send(AppMsg::MonitorEdited(Ok(updated_monitor)));
            }
            Err(error) => {
                let restore = reconnect_after_edit_exit(client, session.original_monitor.clone())
                    .await
                    .map_err(|restore_error| restore_error.to_string());
                match restore {
                    Ok((monitor_id, label, new_rule_id)) => {
                        let _ = msg_tx.send(AppMsg::MonitorReconnected(Ok((
                            monitor_id,
                            label,
                            new_rule_id,
                        ))));
                        let _ = msg_tx.send(AppMsg::MonitorEdited(Err(format!(
                            "{error}; original target was restored"
                        ))));
                    }
                    Err(restore_error) => {
                        let _ = msg_tx.send(AppMsg::MonitorEdited(Err(format!(
                            "{error}; restore failed: {restore_error}"
                        ))));
                    }
                }
            }
        }
    });
}

fn edit_selected_monitor(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(client) = x_client else {
        app.push_error("cannot edit target without 𝕏 bearer token");
        return;
    };

    let Some(monitor) = app.selected_monitor().cloned() else {
        app.push_info("no monitor selected");
        return;
    };

    app.set_monitor_active(monitor.id, false);
    app.push_info(format!(
        "disconnecting target '{}' before edit...",
        monitor.label
    ));

    tokio::spawn(async move {
        let result = disconnect_monitor_for_edit(client, monitor.clone())
            .await
            .map_err(|error| error.to_string());
        let _ = msg_tx.send(AppMsg::MonitorEditPrepared {
            monitor_id: monitor.id,
            result,
        });
    });
}

fn cancel_monitor_edit(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(session) = app.edit_session.clone() else {
        app.close_add_form();
        return;
    };

    let Some(client) = x_client else {
        app.push_error("cannot reconnect target without 𝕏 bearer token");
        app.close_add_form();
        return;
    };

    let monitor = session.original_monitor;
    app.close_add_form();
    app.set_monitor_active(monitor.id, false);
    app.push_info(format!(
        "reconnecting target '{}' after edit cancel...",
        monitor.label
    ));

    tokio::spawn(async move {
        let result = reconnect_after_edit_exit(client, monitor)
            .await
            .map_err(|error| error.to_string());
        let _ = msg_tx.send(AppMsg::MonitorReconnected(result));
    });
}

async fn create_monitor(client: XApiClient, pending: PendingMonitor) -> Result<Monitor> {
    let rule_id = client
        .add_rule(pending.query.clone(), pending.rule_tag.clone())
        .await
        .context("x rule creation failed")?;

    Ok(Monitor {
        id: pending.id,
        label: pending.label,
        kind: pending.kind,
        enabled: pending.enabled,
        input_value: pending.input_value,
        query: pending.query,
        rule_id,
        rule_tag: pending.rule_tag,
        analysis: pending.analysis,
        created_at: Utc::now(),
    })
}

fn toggle_selected_monitor_activation(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(client) = x_client else {
        app.push_error("cannot toggle target without 𝕏 bearer token");
        return;
    };

    let Some(monitor) = app.selected_monitor().cloned() else {
        app.push_info("no monitor selected");
        return;
    };

    if monitor.enabled {
        app.set_monitor_active(monitor.id, false);
        app.push_info(format!("deactivating target '{}'...", monitor.label));
        tokio::spawn(async move {
            let result = deactivate_monitor_rule(client, monitor)
                .await
                .map_err(|error| error.to_string());
            let _ = msg_tx.send(AppMsg::MonitorDeactivated(result));
        });
    } else {
        app.set_monitor_active(monitor.id, false);
        app.push_info(format!("activating target '{}'...", monitor.label));
        tokio::spawn(async move {
            let result = activate_monitor_rule(client, monitor)
                .await
                .map_err(|error| error.to_string());
            let _ = msg_tx.send(AppMsg::MonitorActivated(result));
        });
    }
}

fn delete_selected_monitor(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(client) = x_client else {
        app.push_error("cannot delete monitor without 𝕏 bearer token");
        return;
    };

    let Some(monitor) = app.selected_monitor().cloned() else {
        app.push_info("no monitor selected");
        return;
    };

    app.push_info(format!("removing monitor '{}'...", monitor.label));

    tokio::spawn(async move {
        let result = if monitor.rule_id.trim().is_empty() {
            Ok((monitor.id, monitor.label))
        } else {
            client
                .delete_rule(monitor.rule_id.clone())
                .await
                .map(|_| (monitor.id, monitor.label))
                .map_err(|error| error.to_string())
        };
        let _ = msg_tx.send(AppMsg::MonitorDeleted(result));
    });
}

fn reconnect_selected_monitor(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(client) = x_client else {
        app.push_error("cannot reconnect target without 𝕏 bearer token");
        return;
    };

    let Some(monitor) = app.selected_monitor().cloned() else {
        app.push_info("no monitor selected");
        return;
    };

    app.set_monitor_active(monitor.id, false);
    app.push_info(format!("reconnecting target '{}'...", monitor.label));

    tokio::spawn(async move {
        let result = reconnect_monitor_rule(client, monitor)
            .await
            .map_err(|error| error.to_string());
        let _ = msg_tx.send(AppMsg::MonitorReconnected(result));
    });
}

async fn reconnect_monitor_rule(
    client: XApiClient,
    monitor: Monitor,
) -> Result<(Uuid, String, String)> {
    if !monitor.rule_id.trim().is_empty() {
        if let Err(error) = client.delete_rule(monitor.rule_id.clone()).await {
            if !is_rule_not_found_error(&error) {
                return Err(error).context("x rule deletion failed during reconnect");
            }
        }
    }

    let new_rule_id = client
        .add_rule(monitor.query.clone(), monitor.rule_tag.clone())
        .await
        .context("x rule creation failed during reconnect")?;

    Ok((monitor.id, monitor.label, new_rule_id))
}

async fn deactivate_monitor_rule(client: XApiClient, monitor: Monitor) -> Result<(Uuid, String)> {
    if monitor.rule_id.trim().is_empty() {
        return Ok((monitor.id, monitor.label));
    }

    if let Err(error) = client.delete_rule(monitor.rule_id.clone()).await {
        if !is_rule_not_found_error(&error) {
            return Err(error).context("x rule deletion failed while deactivating target");
        }
    }

    Ok((monitor.id, monitor.label))
}

async fn activate_monitor_rule(
    client: XApiClient,
    monitor: Monitor,
) -> Result<(Uuid, String, String)> {
    let new_rule_id = client
        .add_rule(monitor.query.clone(), monitor.rule_tag.clone())
        .await
        .context("x rule creation failed while activating target")?;

    Ok((monitor.id, monitor.label, new_rule_id))
}

async fn disconnect_monitor_for_edit(client: XApiClient, monitor: Monitor) -> Result<Monitor> {
    if let Err(error) = client.delete_rule(monitor.rule_id.clone()).await {
        if !is_rule_not_found_error(&error) {
            return Err(error).context("x rule deletion failed before edit");
        }
    }

    Ok(monitor)
}

async fn reconnect_after_edit_exit(
    client: XApiClient,
    monitor: Monitor,
) -> Result<(Uuid, String, String)> {
    let new_rule_id = client
        .add_rule(monitor.query.clone(), monitor.rule_tag.clone())
        .await
        .context("x rule creation failed while reconnecting after edit")?;

    Ok((monitor.id, monitor.label, new_rule_id))
}

fn is_rule_not_found_error(error: &anyhow::Error) -> bool {
    let err_text = error.to_string().to_ascii_lowercase();
    err_text.contains("404") || err_text.contains("not found")
}

fn terminate_all_connections(
    app: &mut App,
    msg_tx: mpsc::UnboundedSender<AppMsg>,
    x_client: Option<XApiClient>,
) {
    let Some(client) = x_client else {
        app.push_error("cannot terminate connections without 𝕏 bearer token");
        return;
    };

    app.set_stream_connected(false);
    app.push_info("terminating all filtered stream connections...");

    tokio::spawn(async move {
        match client.terminate_all_connections().await {
            Ok(summary) => {
                let _ = msg_tx.send(AppMsg::Info(summary));
            }
            Err(error) => {
                let _ = msg_tx.send(AppMsg::Error(format!(
                    "failed to terminate stream connections: {error}"
                )));
            }
        }
    });
}

fn open_selected_feed_url(app: &mut App) {
    let Some(item) = app.selected_feed_item() else {
        app.push_info("no feed item selected");
        return;
    };

    let Some(url) = item.url.clone() else {
        app.push_info("selected feed item has no URL");
        return;
    };

    match webbrowser::open(&url) {
        Ok(_) => app.push_info(format!("opened {url}")),
        Err(error) => app.push_error(format!("failed to open URL: {error}")),
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn resolve_api_key_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(var_name) = trimmed.strip_prefix('$') {
        if is_env_var_name(var_name) {
            return std::env::var(var_name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }
        return Some(trimmed.to_string());
    }

    if is_env_var_name(trimmed) {
        return std::env::var(trimmed)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    Some(trimmed.to_string())
}

fn is_env_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

fn prepare_monitor_config_dir(config: &mut config::AppConfig) -> Result<()> {
    if config.monitor_config_dir.is_relative() {
        config.monitor_config_dir = std::env::current_dir()?.join(&config.monitor_config_dir);
    }

    fs::create_dir_all(&config.monitor_config_dir).with_context(|| {
        format!(
            "failed to create monitor config directory {}",
            config.monitor_config_dir.display()
        )
    })?;

    let example_path = config.monitor_config_dir.join("example-account.yaml");
    if !example_path.exists() {
        fs::write(&example_path, SAMPLE_TARGET_FILE).with_context(|| {
            format!(
                "failed to write example target file at {}",
                example_path.display()
            )
        })?;
    }

    Ok(())
}

const SAMPLE_TARGET_FILE: &str = r#"label: "Example account watch"
kind: account
target: "@handle_1, handle2, @handle_3"
ai:
  enabled: true
  provider: grok
  model: grok-4-1-fast-non-reasoning
  prompt: "Summarize why this post matters and what to watch next."
"#;
