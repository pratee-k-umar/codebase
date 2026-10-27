//! Application state and screens.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Ui};

use codebase_intel_core::Language;

use crate::block::{BlockDiagram, BlockView};
use crate::graph::{kind_tag, language_color, Graph, GraphView, ScopeMode};
use crate::repo;
use crate::scan::{self, ScanOutcome, ScanUpdate};
use crate::settings::Settings;

const ACCENT: Color32 = Color32::from_rgb(94, 168, 255);
const DANGER: Color32 = Color32::from_rgb(232, 106, 106);
const MUTED: Color32 = Color32::from_rgb(140, 148, 162);
const SUCCESS: Color32 = Color32::from_rgb(110, 200, 140);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultsTab {
    Graph,
    Blocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Welcome,
    Clone,
    OpenFolder,
    Settings,
    Scanning,
    Graph,
}

pub struct App {
    screen: Screen,
    settings: Settings,
    excludes_text: String,

    clone_url: String,
    clone_parent: String,
    clone_name: String,
    clone_name_touched: bool,
    clone_error: Option<String>,
    clone_rx: Option<Receiver<Result<PathBuf, String>>>,
    cloning: bool,

    folder_path: String,
    folder_error: Option<String>,

    status: String,
    scan_rx: Option<Receiver<ScanUpdate>>,
    outcome: Option<ScanOutcome>,
    error: Option<String>,

    view: Option<GraphView>,
    blocks: Option<BlockView>,
    blocks_dirty: bool,
    tab: ResultsTab,
    scope_search: String,
    toast: Option<(String, Instant)>,
}

impl App {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        configure_style(&context.egui_ctx);

        let settings = Settings::load();
        let clone_parent = settings
            .clone_parent
            .clone()
            .or_else(default_clone_parent)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();

        let folder_path = settings
            .last_repo
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();

        let excludes_text = settings.extra_excludes.join("\n");

        Self {
            screen: Screen::Welcome,
            settings,
            excludes_text,
            clone_url: String::new(),
            clone_parent,
            clone_name: String::new(),
            clone_name_touched: false,
            clone_error: None,
            clone_rx: None,
            cloning: false,
            folder_path,
            folder_error: None,
            status: String::new(),
            scan_rx: None,
            outcome: None,
            error: None,
            view: None,
            blocks: None,
            blocks_dirty: true,
            tab: ResultsTab::Graph,
            scope_search: String::new(),
            toast: None,
        }
    }

    fn save_settings(&mut self) {
        if let Err(err) = self.settings.save() {
            self.toast = Some((err, Instant::now()));
        }
    }

    fn start_scan(&mut self, root: PathBuf) {
        self.settings.last_repo = Some(root.clone());
        self.save_settings();

        self.error = None;
        self.outcome = None;
        self.view = None;
        self.status = "Starting…".to_string();
        self.scan_rx = Some(scan::spawn(root, self.settings.to_config()));
        self.screen = Screen::Scanning;
    }

    fn start_clone(&mut self) {
        self.clone_error = None;

        let url = self.clone_url.trim().to_string();
        let parent_text = self.clone_parent.trim().to_string();

        if url.is_empty() {
            self.clone_error = Some("Enter a repository URL.".to_string());
            return;
        }
        if parent_text.is_empty() {
            self.clone_error = Some("Choose where the repository should be cloned.".to_string());
            return;
        }

        let name = if self.clone_name.trim().is_empty() {
            match repo::repo_name_from_url(&url) {
                Some(derived) => derived,
                None => {
                    self.clone_error = Some("Enter a destination folder name.".to_string());
                    return;
                }
            }
        } else {
            self.clone_name.trim().to_string()
        };

        let parent = PathBuf::from(parent_text);
        self.settings.clone_parent = Some(parent.clone());
        self.save_settings();

        let (sender, receiver) = mpsc::channel();
        self.clone_rx = Some(receiver);
        self.cloning = true;

        thread::spawn(move || {
            let outcome = repo::clone_repository(&url, &parent, &name);
            let _ = sender.send(outcome);
        });
    }

    fn poll_background(&mut self, context: &egui::Context) {
        if let Some(receiver) = &self.scan_rx {
            let mut finished = false;
            let mut failure = None;

            while let Ok(message) = receiver.try_recv() {
                match message {
                    ScanUpdate::Progress(stage) => self.status = stage,
                    ScanUpdate::Finished(outcome) => {
                        self.status = format!("Done in {}", scan::format_duration(outcome.elapsed));
                        self.toast = Some((
                            format!(
                                "{} files · {} symbols · {} relations",
                                outcome.result.file_dependencies.nodes.len(),
                                outcome.result.symbols.len(),
                                outcome.result.file_dependencies.edges.len()
                            ),
                            Instant::now(),
                        ));
                        self.view = Some(GraphView::new(Graph::build(&outcome.result)));
                        self.blocks = None;
                        self.blocks_dirty = true;
                        self.tab = ResultsTab::Graph;
                        self.outcome = Some(*outcome);
                        self.screen = Screen::Graph;
                        finished = true;
                    }
                    ScanUpdate::Failed(message) => {
                        failure = Some(message);
                        finished = true;
                    }
                }
            }

            if let Some(message) = failure {
                self.screen = Screen::Welcome;
                self.error = Some(format!("Analysis failed: {message}"));
            }

            if finished {
                self.scan_rx = None;
            } else {
                context.request_repaint_after(Duration::from_millis(80));
            }
        }

        if let Some(receiver) = &self.clone_rx {
            match receiver.try_recv() {
                Ok(Ok(path)) => {
                    self.clone_rx = None;
                    self.cloning = false;
                    self.start_scan(path);
                }
                Ok(Err(message)) => {
                    self.clone_rx = None;
                    self.cloning = false;
                    self.clone_error = Some(message);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.clone_rx = None;
                    self.cloning = false;
                    self.clone_error = Some("Clone stopped unexpectedly.".to_string());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    context.request_repaint_after(Duration::from_millis(120));
                }
            }
        }

        if let Some((_, shown_at)) = &self.toast {
            if shown_at.elapsed() > Duration::from_secs(6) {
                self.toast = None;
            } else {
                context.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    // ----- screens -------------------------------------------------------

    fn welcome(&mut self, ui: &mut Ui) {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Codebase Intel").size(34.0).strong());
            ui.add_space(6.0);
            ui.label(
                RichText::new("Map a repository before you change it.")
                    .size(14.0)
                    .color(MUTED),
            );
        });

        ui.add_space(38.0);

        ui.vertical_centered(|ui| {
            if option_button(
                ui,
                "Clone a repository",
                "Download a git repository, then analyze it",
            )
            .clicked()
            {
                self.screen = Screen::Clone;
            }
            ui.add_space(12.0);

            if option_button(ui, "Open a local folder", "Analyze a repository already on disk")
                .clicked()
            {
                self.folder_error = None;
                self.screen = Screen::OpenFolder;
            }
            ui.add_space(12.0);

            if option_button(ui, "Settings", "Languages, excludes and cache").clicked() {
                self.screen = Screen::Settings;
            }

            if let Some(last) = self.settings.last_repo.clone() {
                if last.exists() {
                    ui.add_space(26.0);
                    ui.label(RichText::new("Recent").color(MUTED));
                    let label = format!(
                        "{}  ·  {}",
                        last.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| last.to_string_lossy().into_owned()),
                        last.display()
                    );
                    if ui
                        .add_sized(
                            [460.0, 34.0],
                            egui::Button::new(RichText::new(label).size(13.0)),
                        )
                        .clicked()
                    {
                        self.start_scan(last);
                    }
                }
            }
        });

        if let Some(error) = self.error.clone() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(error).color(DANGER));
            });
        }
    }

    fn clone_screen(&mut self, ui: &mut Ui) {
        if back_button(ui) {
            self.screen = Screen::Welcome;
            return;
        }

        ui.heading("Clone a repository");
        ui.label(
            RichText::new("The repository is cloned with a shallow history, then analyzed.")
                .color(MUTED),
        );
        ui.add_space(18.0);

        ui.label("Repository URL");
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.clone_url)
                .hint_text("https://github.com/owner/repo.git")
                .desired_width(460.0),
        );
        if response.changed() && !self.clone_name_touched {
            if let Some(name) = repo::repo_name_from_url(&self.clone_url) {
                self.clone_name = name;
            }
        }

        ui.add_space(14.0);
        ui.label("Clone into");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.clone_parent)
                    .desired_width(380.0)
                    .hint_text("Choose a folder…"),
            );
            if ui.button("Browse…").clicked() {
                let mut dialog = rfd::FileDialog::new();
                if let Some(current) = existing_directory(&self.clone_parent) {
                    dialog = dialog.set_directory(current);
                }
                if let Some(folder) = dialog.pick_folder() {
                    self.clone_parent = folder.to_string_lossy().into_owned();
                }
            }
        });

        ui.add_space(14.0);
        ui.label("Folder name");
        let name = ui.add(
            egui::TextEdit::singleline(&mut self.clone_name).desired_width(260.0),
        );
        if name.changed() {
            self.clone_name_touched = true;
        }

        ui.add_space(10.0);
        let parent = PathBuf::from(self.clone_parent.trim());
        let preview = if parent.as_os_str().is_empty() {
            "(choose a destination)".to_string()
        } else {
            parent
                .join(if self.clone_name.trim().is_empty() {
                    "<repo>"
                } else {
                    self.clone_name.trim()
                })
                .display()
                .to_string()
        };
        ui.label(RichText::new(preview).color(MUTED).size(12.0));

        ui.add_space(20.0);
        ui.horizontal(|ui| {
            let can_clone = !self.cloning;
            if ui
                .add_enabled(
                    can_clone,
                    egui::Button::new(RichText::new("Clone and analyze").size(15.0))
                        .min_size(egui::vec2(180.0, 34.0)),
                )
                .clicked()
            {
                self.start_clone();
            }

            if self.cloning {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Cloning…").color(MUTED));
            }
        });

        if let Some(error) = &self.clone_error {
            ui.add_space(14.0);
            ui.label(RichText::new(error).color(DANGER));
        }
    }

    fn open_screen(&mut self, ui: &mut Ui) {
        if back_button(ui) {
            self.screen = Screen::Welcome;
            return;
        }

        ui.heading("Open a local folder");
        ui.label(RichText::new("Point at a repository that is already on disk.").color(MUTED));
        ui.add_space(18.0);

        ui.label("Folder");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.folder_path)
                    .desired_width(420.0)
                    .hint_text("C:\\path\\to\\repository"),
            );
            if ui.button("Browse…").clicked() {
                let mut dialog = rfd::FileDialog::new();
                if let Some(current) = existing_directory(&self.folder_path) {
                    dialog = dialog.set_directory(current);
                }
                if let Some(folder) = dialog.pick_folder() {
                    self.folder_path = folder.to_string_lossy().into_owned();
                }
            }
        });

        ui.add_space(20.0);
        if ui
            .add(
                egui::Button::new(RichText::new("Analyze folder").size(15.0))
                    .min_size(egui::vec2(160.0, 34.0)),
            )
            .clicked()
        {
            let path = PathBuf::from(self.folder_path.trim());
            match repo::validate_repository(&path) {
                Ok(()) => {
                    self.folder_error = None;
                    self.start_scan(path);
                }
                Err(message) => self.folder_error = Some(message),
            }
        }

        if let Some(error) = &self.folder_error {
            ui.add_space(14.0);
            ui.label(RichText::new(error).color(DANGER));
        }
    }

    fn settings_screen(&mut self, ui: &mut Ui) {
        if back_button(ui) {
            self.screen = if self.view.is_some() {
                Screen::Graph
            } else {
                Screen::Welcome
            };
            return;
        }

        ui.heading("Settings");
        ui.add_space(14.0);

        ui.label(RichText::new("Languages").strong());
        ui.label(
            RichText::new("Unchecking a language means its files are not analyzed.")
                .color(MUTED)
                .size(12.0),
        );
        ui.add_space(6.0);

        ui.horizontal_wrapped(|ui| {
            for language in [
                Language::TypeScript,
                Language::JavaScript,
                Language::Python,
                Language::Go,
                Language::Rust,
            ] {
                let mut enabled = self.settings.is_language_enabled(language);
                if ui.checkbox(&mut enabled, language_label(language)).changed() {
                    self.settings.toggle_language(language);
                }
                ui.add_space(10.0);
            }
        });

        ui.add_space(18.0);
        ui.label(RichText::new("Extra exclude patterns").strong());
        ui.label(
            RichText::new("One glob per line, added to the built-in excludes.")
                .color(MUTED)
                .size(12.0),
        );
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.excludes_text)
                .desired_rows(5)
                .desired_width(460.0)
                .hint_text("**/fixtures/**\n**/*.generated.ts"),
        );

        ui.add_space(16.0);
        ui.checkbox(&mut self.settings.cache_enabled, "Use the analysis cache");

        ui.add_space(22.0);
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("Save").size(15.0)).min_size(egui::vec2(110.0, 32.0)))
                .clicked()
            {
                self.settings.extra_excludes = self
                    .excludes_text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect();
                self.save_settings();
                self.toast = Some(("Settings saved".to_string(), Instant::now()));
            }

            if ui.button("Close").clicked() {
                self.screen = if self.view.is_some() {
                    Screen::Graph
                } else {
                    Screen::Welcome
                };
            }
        });

        ui.add_space(24.0);
        ui.label(
            RichText::new(format!("Saved to {}", Settings::path().display()))
                .color(MUTED)
                .size(11.0),
        );
    }

    fn scanning_screen(&mut self, ui: &mut Ui) {
        ui.add_space(120.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(34.0));
            ui.add_space(18.0);
            ui.label(RichText::new(&self.status).size(17.0));
            ui.add_space(6.0);
            ui.label(
                RichText::new("Large repositories can take a little while.")
                    .color(MUTED)
                    .size(12.0),
            );
        });
    }

    fn results_screen(&mut self, context: &egui::Context) {
        egui::SidePanel::left("scope")
            .resizable(true)
            .default_width(250.0)
            .width_range(190.0..=430.0)
            .show(context, |ui| self.scope_panel(ui));

        egui::SidePanel::right("details")
            .resizable(true)
            .default_width(340.0)
            .width_range(260.0..=560.0)
            .show(context, |ui| self.details_panel(ui));

        let mut export = false;

        egui::CentralPanel::default().show(context, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, ResultsTab::Graph, "Graph");
                ui.selectable_value(&mut self.tab, ResultsTab::Blocks, "Block diagram");
                ui.separator();

                match self.tab {
                    ResultsTab::Graph => {
                        if let Some(view) = &mut self.view {
                            ui.add(
                                egui::TextEdit::singleline(&mut view.filter)
                                    .hint_text("Highlight files...")
                                    .desired_width(170.0),
                            );
                            if ui.button("Fit").clicked() {
                                view.fit_requested = true;
                            }
                            if ui.button("Re-layout").clicked() {
                                view.graph.layout();
                                view.fit_requested = true;
                            }
                        }
                    }
                    ResultsTab::Blocks => {
                        if let Some(blocks) = &mut self.blocks {
                            if ui.button("Fit").clicked() {
                                blocks.fit_requested = true;
                            }
                            if ui.button("Re-layout").clicked() {
                                blocks.diagram.layout();
                                blocks.fit_requested = true;
                            }
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Export JSON...").clicked() {
                        export = true;
                    }
                    if let Some(outcome) = &self.outcome {
                        ui.label(
                            RichText::new(format!(
                                "{} files | {} symbols | {} relations",
                                outcome.result.file_dependencies.nodes.len(),
                                outcome.result.symbols.len(),
                                outcome.result.file_dependencies.edges.len()
                            ))
                            .color(MUTED)
                            .size(12.0),
                        );
                    }
                });
            });

            ui.separator();

            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                if let Some(view) = &mut self.view {
                    view.selected = None;
                }
            }

            match self.tab {
                ResultsTab::Graph => {
                    if let Some(view) = &mut self.view {
                        view.ui(ui);
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new("Nothing analyzed yet.").color(MUTED));
                        });
                    }
                }
                ResultsTab::Blocks => {
                    self.ensure_blocks();
                    if let Some(blocks) = &mut self.blocks {
                        blocks.ui(ui);
                        if let Some(id) = blocks.selected {
                            if let Some(view) = &mut self.view {
                                view.selected = Some(id);
                            }
                        }
                    }
                }
            }
        });

        if export {
            self.export_json();
        }
    }

    /// Rebuild the block diagram when the scope or the analysis changed.
    fn ensure_blocks(&mut self) {
        if !self.blocks_dirty && self.blocks.is_some() {
            return;
        }
        let Some(outcome) = &self.outcome else {
            return;
        };

        let visible = self.view.as_ref().and_then(|view| view.visible_ids());
        let entry = self
            .view
            .as_ref()
            .map(|view| view.graph.entry_points.clone())
            .unwrap_or_default();

        let diagram = BlockDiagram::build(&outcome.result, visible.as_ref(), &entry);
        self.blocks = Some(BlockView::new(diagram));
        self.blocks_dirty = false;
    }

    fn scope_panel(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Scope").strong().size(14.0));

        // The search field is edited before the graph is borrowed mutably.
        ui.add(
            egui::TextEdit::singleline(&mut self.scope_search)
                .hint_text("Find a file...")
                .desired_width(f32::INFINITY),
        );
        let needle = self.scope_search.trim().to_ascii_lowercase();

        let Some(view) = &mut self.view else {
            ui.label(RichText::new("Run a scan first.").color(MUTED));
            return;
        };

        ui.horizontal(|ui| {
            ui.selectable_value(&mut view.scope.mode, ScopeMode::All, "All");
            ui.selectable_value(&mut view.scope.mode, ScopeMode::Neighborhood, "Near");
            ui.selectable_value(&mut view.scope.mode, ScopeMode::SelectionOnly, "Only");
        });

        if view.scope.mode == ScopeMode::Neighborhood {
            ui.horizontal(|ui| {
                ui.label(RichText::new("hops").size(11.0).color(MUTED));
                ui.add(egui::Slider::new(&mut view.scope.hops, 1..=3));
            });
        }

        ui.horizontal(|ui| {
            if ui.button("Select all").clicked() {
                view.scope.selected = view.graph.nodes.iter().map(|node| node.id).collect();
                view.scope.mode = ScopeMode::SelectionOnly;
                view.fit_requested = true;
            }
            if ui.button("Clear").clicked() {
                view.scope.selected.clear();
                view.scope.mode = ScopeMode::All;
                view.fit_requested = true;
            }
        });

        ui.label(
            RichText::new(format!(
                "{} of {} files ticked",
                view.scope.selected.len(),
                view.graph.nodes.len()
            ))
            .color(MUTED)
            .size(11.0),
        );

        ui.add_space(4.0);

        let indices: Vec<usize> = view
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                needle.is_empty()
                    || node.label.to_ascii_lowercase().contains(&needle)
                    || node.directory.to_ascii_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect();

        let mut toggled: Option<(usize, bool)> = None;
        let mut clicked: Option<usize> = None;
        let mut changed = false;

        let rows_height = (ui.available_height() - 160.0).max(120.0);
        egui::ScrollArea::vertical()
            .id_salt("scope_files")
            .max_height(rows_height)
            .show_rows(ui, 18.0, indices.len(), |ui, range| {
                for position in range {
                    let node_index = indices[position];
                    let (id, label, directory) = {
                        let node = &view.graph.nodes[node_index];
                        (node.id, node.label.clone(), node.directory.clone())
                    };
                    let mut checked = view.scope.selected.contains(&id);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut checked, "").changed() {
                            toggled = Some((id, checked));
                        }
                        let text = RichText::new(format!("{directory}/{label}")).size(11.0);
                        if ui.selectable_label(view.selected == Some(id), text).clicked() {
                            clicked = Some(id);
                        }
                    });
                }
            });

        if let Some((id, on)) = toggled {
            if on {
                view.scope.selected.insert(id);
            } else {
                view.scope.selected.remove(&id);
            }
            if !view.scope.selected.is_empty() && view.scope.mode == ScopeMode::All {
                view.scope.mode = ScopeMode::Neighborhood;
                view.scope.hops = 1;
            }
            view.fit_requested = true;
            changed = true;
        }

        if let Some(id) = clicked {
            view.selected = Some(id);
            let canvas = view.last_canvas;
            view.focus(canvas, id);
            changed = true;
        }

        ui.separator();
        ui.label(RichText::new("Starting points").strong().size(12.0));
        ui.label(
            RichText::new("Files that nothing imports, but which import others.")
                .color(MUTED)
                .size(10.5),
        );

        let entries = view.graph.entry_points.clone();
        let mut jump: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("entries")
            .max_height(120.0)
            .show(ui, |ui| {
                for id in &entries {
                    if let Some(node) = view.graph.node(*id) {
                        if ui
                            .link(RichText::new(format!("start  {}", node.label)).size(11.5))
                            .clicked()
                        {
                            jump = Some(*id);
                        }
                    }
                }
                if entries.is_empty() {
                    ui.label(RichText::new("None detected.").color(MUTED).size(11.0));
                }
            });

        if let Some(id) = jump {
            view.selected = Some(id);
            let canvas = view.last_canvas;
            view.focus(canvas, id);
        }

        if changed {
            self.blocks_dirty = true;
        }
    }
    fn details_panel(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);

        let selected = self.view.as_ref().and_then(|view| view.selected);

        let Some(id) = selected else {
            self.summary_panel(ui);
            return;
        };

        let Some(view) = &self.view else { return };
        let Some(node) = view.graph.node(id) else { return };

        let name = node.label.clone();
        let path = node.path.clone();
        let directory = node.directory.clone();
        let language = node.language;
        let total_symbols = node.total_symbols;
        let inbound = node.inbound;
        let outbound = node.outbound;

        let (incoming, outgoing) = view.graph.relations(id);
        let incoming: Vec<(usize, String, &'static str)> = incoming
            .iter()
            .filter_map(|edge| {
                view.graph
                    .node(edge.from)
                    .map(|node| (node.id, node.label.clone(), edge.kind_label()))
            })
            .collect();
        let outgoing: Vec<(usize, String, &'static str)> = outgoing
            .iter()
            .filter_map(|edge| {
                view.graph
                    .node(edge.to)
                    .map(|node| (node.id, node.label.clone(), edge.kind_label()))
            })
            .collect();

        let symbols: Vec<(String, &'static str, usize)> = self
            .outcome
            .as_ref()
            .map(|outcome| {
                let target = codebase_intel_core::resolve::normalize(&path);
                outcome
                    .result
                    .symbols
                    .iter()
                    .filter(|symbol| codebase_intel_core::resolve::normalize(&symbol.file) == target)
                    .map(|symbol| {
                        (
                            symbol.name.clone(),
                            kind_tag(symbol.kind),
                            symbol.span.start.line,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        ui.horizontal(|ui| {
            ui.label(RichText::new(&name).strong().size(16.0));
            ui.label(
                RichText::new(language_label(language))
                    .color(language_color(language))
                    .size(11.0),
            );
        });
        ui.label(RichText::new(&directory).color(MUTED).size(12.0));
        ui.label(
            RichText::new(path.display().to_string())
                .color(MUTED)
                .size(10.5),
        );

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            stat(ui, "symbols", total_symbols);
            stat(ui, "imports", inbound);
            stat(ui, "imported by", outbound);
        });

        ui.add_space(10.0);
        ui.separator();

        ui.label(RichText::new(format!("Symbols ({total_symbols})")).strong());
        egui::ScrollArea::vertical()
            .id_salt("symbols")
            .max_height(220.0)
            .show(ui, |ui| {
                for (name, tag, line) in &symbols {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(*tag)
                                .monospace()
                                .size(10.0)
                                .color(MUTED),
                        );
                        ui.label(RichText::new(name).size(12.5));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(RichText::new(format!(":{line}")).color(MUTED).size(10.5));
                            },
                        );
                    });
                }
                if symbols.is_empty() {
                    ui.label(RichText::new("No symbols in this file.").color(MUTED).size(12.0));
                }
            });

        ui.add_space(8.0);
        ui.separator();

        let mut jump: Option<usize> = None;

        ui.label(RichText::new(format!("Depends on ({outbound})")).strong());
        egui::ScrollArea::vertical()
            .id_salt("outgoing")
            .max_height(120.0)
            .show(ui, |ui| {
                for (target, label, kind) in &outgoing {
                    ui.horizontal(|ui| {
                        if ui.link(label).clicked() {
                            jump = Some(*target);
                        }
                        ui.label(RichText::new(*kind).color(MUTED).size(10.0));
                    });
                }
                if outgoing.is_empty() {
                    ui.label(RichText::new("Nothing resolved.").color(MUTED).size(12.0));
                }
            });

        ui.add_space(6.0);
        ui.label(RichText::new(format!("Imported by ({inbound})")).strong());
        egui::ScrollArea::vertical()
            .id_salt("incoming")
            .max_height(120.0)
            .show(ui, |ui| {
                for (source, label, kind) in &incoming {
                    ui.horizontal(|ui| {
                        if ui.link(label).clicked() {
                            jump = Some(*source);
                        }
                        ui.label(RichText::new(*kind).color(MUTED).size(10.0));
                    });
                }
                if incoming.is_empty() {
                    ui.label(RichText::new("Nothing resolved.").color(MUTED).size(12.0));
                }
            });

        if let Some(target) = jump {
            let canvas = self
                .view
                .as_ref()
                .map(|view| view.last_canvas)
                .unwrap_or(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                ));
            if let Some(view) = &mut self.view {
                view.focus(canvas, target);
            }
        }
    }

    fn summary_panel(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Analysis").strong().size(16.0));
        ui.add_space(6.0);

        let Some(outcome) = &self.outcome else {
            ui.label(RichText::new("Nothing analyzed yet.").color(MUTED));
            return;
        };

        ui.label(
            RichText::new(outcome.root.display().to_string())
                .color(MUTED)
                .size(10.5),
        );
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            stat(ui, "files", outcome.result.file_dependencies.nodes.len());
            stat(ui, "symbols", outcome.result.symbols.len());
            stat(
                ui,
                "relations",
                outcome.result.file_dependencies.edges.len(),
            );
        });
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("Scanned in {}", scan::format_duration(outcome.elapsed)))
                .color(MUTED)
                .size(11.5),
        );

        let discovery = &outcome.result.discovery;
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new("Discovery").strong());
        kv(ui, "files scanned", discovery.files_scanned);
        kv(ui, "included", discovery.files_included);
        kv(ui, "ignored", discovery.files_ignored);
        kv(ui, "directories pruned", discovery.dirs_pruned);
        kv(ui, "unsupported", discovery.files_unsupported);

        if !discovery.ignored_by_reason.is_empty() {
            ui.add_space(6.0);
            for (reason, count) in &discovery.ignored_by_reason {
                ui.label(
                    RichText::new(format!("{} · {count}", reason.as_str()))
                        .color(MUTED)
                        .size(11.5),
                );
            }
        }

        if !outcome.result.metrics.languages.is_empty() {
            ui.add_space(12.0);
            ui.separator();
            ui.label(RichText::new("Languages").strong());
            for (language, count) in &outcome.result.metrics.languages {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(language_label(*language))
                            .color(language_color(*language))
                            .size(12.0),
                    );
                    ui.label(RichText::new(format!("{count} symbols")).color(MUTED).size(11.5));
                });
            }
        }

        ui.add_space(12.0);
        ui.label(
            RichText::new("Click a file to inspect it. Only imports that resolve to another scanned file become relations.")
                .color(MUTED)
                .size(11.0),
        );
    }

    fn export_json(&mut self) {
        let Some(outcome) = &self.outcome else { return };

        let Some(path) = rfd::FileDialog::new()
            .set_file_name("analysis.json")
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };

        let mut buffer = Vec::new();
        match codebase_intel_core::export::export(
            &outcome.result,
            codebase_intel_core::OutputFormat::Json,
            &mut buffer,
        ) {
            Ok(()) => match std::fs::write(&path, buffer) {
                Ok(()) => {
                    self.toast = Some((
                        format!("Wrote {}", path.display()),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    self.toast = Some((format!("Could not write file: {err}"), Instant::now()));
                }
            },
            Err(err) => {
                self.toast = Some((format!("Export failed: {err}"), Instant::now()));
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background(context);

        if self.screen != Screen::Scanning {
            egui::TopBottomPanel::top("top").show(context, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Codebase Intel").strong().size(16.0));
                    ui.add_space(8.0);

                    if let Some(outcome) = &self.outcome {
                        let name = outcome
                            .root
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| outcome.root.to_string_lossy().into_owned());
                        ui.label(RichText::new(format!("· {name}")).color(MUTED).size(12.0));
                    }

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.button("Settings").clicked() {
                                self.screen = Screen::Settings;
                            }
                            if self.outcome.is_some() && ui.button("Graph").clicked() {
                                self.screen = Screen::Graph;
                            }
                            if ui.button("Start over").clicked() {
                                self.error = None;
                                self.screen = Screen::Welcome;
                            }
                        },
                    );
                });
                ui.add_space(4.0);
            });
        }

        // The graph screen owns its own panels (side panel + central canvas), so
        // it must not also get the generic central panel.
        if self.screen == Screen::Graph {
            self.results_screen(context);
        } else {
            egui::CentralPanel::default().show(context, |ui| match self.screen {
                Screen::Welcome => self.welcome(ui),
                Screen::Clone => self.clone_screen(ui),
                Screen::OpenFolder => self.open_screen(ui),
                Screen::Settings => self.settings_screen(ui),
                Screen::Scanning => self.scanning_screen(ui),
                Screen::Graph => {}
            });
        }

        if let Some((message, _)) = &self.toast {
            egui::Area::new(egui::Id::new("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -18.0))
                .show(context, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(RichText::new(message).color(SUCCESS));
                    });
                });
        }
    }
}

// ----- helpers -----------------------------------------------------------

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(22, 24, 29);
    visuals.window_fill = Color32::from_rgb(26, 29, 35);
    visuals.extreme_bg_color = Color32::from_rgb(16, 18, 22);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(38, 42, 51);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 54, 66);
    visuals.widgets.active.bg_fill = Color32::from_rgb(58, 66, 80);
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.override_text_color = Some(Color32::from_rgb(216, 221, 230));
    context.set_visuals(visuals);

    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    context.set_style(style);
}

fn option_button(ui: &mut Ui, title: &str, description: &str) -> egui::Response {
    ui.add_sized(
        [470.0, 64.0],
        egui::Button::new(
            RichText::new(format!("{title}\n{description}"))
                .size(15.0)
                .line_height(Some(20.0)),
        ),
    )
}

fn back_button(ui: &mut Ui) -> bool {
    let clicked = ui
        .add(egui::Button::new(RichText::new("← Back").size(12.0)).frame(false))
        .clicked();
    ui.add_space(4.0);
    clicked
}

fn stat(ui: &mut Ui, label: &str, value: usize) {
    ui.vertical(|ui| {
        ui.label(RichText::new(value.to_string()).size(18.0).strong());
        ui.label(RichText::new(label).color(MUTED).size(10.5));
    });
    ui.add_space(12.0);
}

fn kv(ui: &mut Ui, label: &str, value: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(MUTED).size(11.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value.to_string()).size(11.5));
        });
    });
}

fn language_label(language: Language) -> &'static str {
    match language {
        Language::TypeScript => "TypeScript",
        Language::JavaScript => "JavaScript",
        Language::Python => "Python",
        Language::Go => "Go",
        Language::Rust => "Rust",
    }
}

fn default_clone_parent() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("Projects"))
        .filter(|path| path.exists())
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .filter(|path| path.exists())
        })
}

fn existing_directory(text: &str) -> Option<PathBuf> {
    let path = Path::new(text.trim());
    path.is_dir().then(|| path.to_path_buf())
}




