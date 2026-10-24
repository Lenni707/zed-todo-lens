mod config;
mod index;
mod scanner;

use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use crossbeam_channel::{select, unbounded, Receiver, Sender};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{self as notif, Notification as _};
use lsp_types::request::{self as req, Request as _};
use lsp_types::*;
use serde_json::json;

use config::Config;
use index::{relative_path, Index, PathFilter};
use scanner::{Scanner, Tag};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Most results the project symbol picker gets for one query.
const SYMBOL_LIMIT: usize = 5000;

fn main() -> Result<()> {
    eprintln!("todo-lens-ls {} starting", env!("CARGO_PKG_VERSION"));
    let (connection, io_threads) = Connection::stdio();

    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let config = Config::from_value(params.initialization_options.as_ref());

    // The token legend is fixed for the lifetime of the server, so it is built from the tags
    // configured at startup. Changing `tags` later needs a server restart to get new colors.
    let legend_tags = config.tags.clone();
    let mut token_types: Vec<SemanticTokenType> =
        legend_tags.iter().map(|t| SemanticTokenType::from(Config::token_type(t))).collect();
    token_types.extend(legend_tags.iter().map(|t| SemanticTokenType::from(Config::text_token_type(t))));

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types,
                    token_modifiers: vec![],
                },
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(false),
                ..Default::default()
            },
        )),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    connection.initialize_finish(
        id,
        json!({
            "capabilities": capabilities,
            "serverInfo": { "name": "todo-lens-ls", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;

    let mut server = Server::new(&connection, &params, config, legend_tags);
    server.run()?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}

/// Messages from background scan threads to the main loop.
enum Internal {
    ScanDone { generation: u64, results: Vec<(PathBuf, Vec<Tag>)> },
}

struct Server<'a> {
    connection: &'a Connection,
    roots: Vec<PathBuf>,
    config: Config,
    legend_tags: Vec<String>,
    scanner: Arc<Scanner>,
    filter: PathFilter,
    index: Index,
    /// Text of documents open in the editor, keyed by path. These win over the disk contents.
    open_docs: HashMap<PathBuf, String>,
    /// Files we have published diagnostics for, so they can be cleared again.
    diagnosed: Vec<PathBuf>,
    supports_watching: bool,
    supports_token_refresh: bool,
    /// Bumped on every rescan so results from an outdated scan are dropped.
    generation: u64,
    next_request_id: i32,
    internal_tx: Sender<Internal>,
    internal_rx: Receiver<Internal>,
}

impl<'a> Server<'a> {
    fn new(connection: &'a Connection, params: &InitializeParams, config: Config, legend_tags: Vec<String>) -> Self {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .iter()
            .flatten()
            .filter_map(|f| uri_to_path(&f.uri))
            .collect();
        #[allow(deprecated)]
        if roots.is_empty() {
            roots.extend(params.root_uri.as_ref().and_then(uri_to_path));
        }
        eprintln!("roots: {roots:?}");

        let caps = &params.capabilities;
        let supports_watching = caps
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            .unwrap_or(false);
        let supports_token_refresh = caps
            .workspace
            .as_ref()
            .and_then(|w| w.semantic_tokens.as_ref())
            .and_then(|s| s.refresh_support)
            .unwrap_or(false);

        let (internal_tx, internal_rx) = unbounded();
        Self {
            connection,
            filter: PathFilter::new(&roots, &config),
            scanner: Arc::new(Scanner::new(&config.tags)),
            roots,
            config,
            legend_tags,
            index: Index::default(),
            open_docs: HashMap::new(),
            diagnosed: Vec::new(),
            supports_watching,
            supports_token_refresh,
            generation: 0,
            next_request_id: 0,
            internal_tx,
            internal_rx,
        }
    }

    fn run(&mut self) -> Result<()> {
        // `initialize_finish` already consumed the `initialized` notification.
        self.register_file_watcher()?;
        self.start_scan();
        loop {
            select! {
                recv(self.connection.receiver) -> msg => {
                    let Ok(msg) = msg else { return Ok(()) };
                    match msg {
                        Message::Request(request) => {
                            if self.connection.handle_shutdown(&request)? {
                                return Ok(());
                            }
                            self.handle_request(request)?;
                        }
                        Message::Notification(n) => self.handle_notification(n)?,
                        Message::Response(_) => {}
                    }
                }
                recv(self.internal_rx) -> msg => {
                    let Ok(Internal::ScanDone { generation, results }) = msg else { continue };
                    if generation != self.generation {
                        continue;
                    }
                    self.index.clear();
                    for (path, tags) in results {
                        self.index.set(path, tags);
                    }
                    // Open buffers may have unsaved edits; keep their version.
                    for (path, text) in &self.open_docs {
                        let tags = index::scan_text(path, text, &self.scanner, &self.config);
                        self.index.set(path.clone(), tags);
                    }
                    let count: usize = self.index.files().map(|(_, t)| t.len()).sum();
                    eprintln!("workspace scan done: {count} tags");
                    self.publish_all_diagnostics()?;
                }
            }
        }
    }

    fn start_scan(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let roots = self.roots.clone();
        let scanner = self.scanner.clone();
        let config = self.config.clone();
        let tx = self.internal_tx.clone();
        std::thread::spawn(move || {
            let results = index::scan_workspace(&roots, &scanner, &config);
            let _ = tx.send(Internal::ScanDone { generation, results });
        });
    }

    fn handle_request(&mut self, request: Request) -> Result<()> {
        let result = match request.method.as_str() {
            req::SemanticTokensFullRequest::METHOD => {
                let params: SemanticTokensParams = serde_json::from_value(request.params)?;
                serde_json::to_value(self.semantic_tokens(&params.text_document.uri))?
            }
            req::WorkspaceSymbolRequest::METHOD => {
                let params: WorkspaceSymbolParams = serde_json::from_value(request.params)?;
                serde_json::to_value(self.workspace_symbols(&params.query))?
            }
            req::DocumentSymbolRequest::METHOD => {
                let params: DocumentSymbolParams = serde_json::from_value(request.params)?;
                serde_json::to_value(self.document_symbols(&params.text_document.uri))?
            }
            _ => {
                let response = Response::new_err(
                    request.id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unhandled method {}", request.method),
                );
                self.connection.sender.send(Message::Response(response))?;
                return Ok(());
            }
        };
        self.connection
            .sender
            .send(Message::Response(Response::new_ok(request.id, result)))?;
        Ok(())
    }

    fn handle_notification(&mut self, n: Notification) -> Result<()> {
        match n.method.as_str() {
            notif::DidOpenTextDocument::METHOD => {
                let p: DidOpenTextDocumentParams = serde_json::from_value(n.params)?;
                self.update_open_doc(&p.text_document.uri, p.text_document.text)?;
            }
            notif::DidChangeTextDocument::METHOD => {
                let p: DidChangeTextDocumentParams = serde_json::from_value(n.params)?;
                // Full sync: the last change holds the whole document.
                if let Some(change) = p.content_changes.into_iter().last() {
                    self.update_open_doc(&p.text_document.uri, change.text)?;
                }
            }
            notif::DidCloseTextDocument::METHOD => {
                let p: DidCloseTextDocumentParams = serde_json::from_value(n.params)?;
                if let Some(path) = uri_to_path(&p.text_document.uri) {
                    self.open_docs.remove(&path);
                    // Fall back to what is on disk (the buffer may have been closed unsaved).
                    self.rescan_from_disk(&path)?;
                }
            }
            notif::DidChangeWatchedFiles::METHOD => {
                let p: DidChangeWatchedFilesParams = serde_json::from_value(n.params)?;
                for change in p.changes {
                    let Some(path) = uri_to_path(&change.uri) else { continue };
                    if self.open_docs.contains_key(&path) {
                        continue;
                    }
                    if change.typ == FileChangeType::DELETED {
                        self.index.remove(&path);
                        self.publish_diagnostics(&path)?;
                    } else if self.filter.is_tracked(&path) {
                        self.rescan_from_disk(&path)?;
                    }
                }
            }
            notif::DidChangeConfiguration::METHOD => {
                let p: DidChangeConfigurationParams = serde_json::from_value(n.params)?;
                self.update_config(Config::from_value(Some(&p.settings)))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn update_open_doc(&mut self, uri: &Uri, text: String) -> Result<()> {
        let Some(path) = uri_to_path(uri) else { return Ok(()) };
        let tags = index::scan_text(&path, &text, &self.scanner, &self.config);
        self.index.set(path.clone(), tags);
        self.open_docs.insert(path.clone(), text);
        self.publish_diagnostics(&path)
    }

    fn rescan_from_disk(&mut self, path: &Path) -> Result<()> {
        match index::scan_file(path, &self.scanner, &self.config) {
            Some(tags) => self.index.set(path.to_path_buf(), tags),
            None => self.index.remove(path),
        }
        self.publish_diagnostics(path)
    }

    fn update_config(&mut self, config: Config) -> Result<()> {
        if config == self.config {
            return Ok(());
        }
        eprintln!("configuration changed: {config:?}");
        if config.tags != self.legend_tags {
            self.show_message(
                MessageType::INFO,
                "Todo Lens: tags changed. Restart the language server (editor: restart language server) to color new tags.",
            )?;
        }
        self.scanner = Arc::new(Scanner::new(&config.tags));
        self.filter = PathFilter::new(&self.roots, &config);
        let diagnostics_were_on = self.config.diagnostics;
        self.config = config;
        if diagnostics_were_on && !self.config.diagnostics {
            self.clear_diagnostics()?;
        }
        self.start_scan();
        self.refresh_tokens()
    }

    fn semantic_tokens(&self, uri: &Uri) -> SemanticTokensResult {
        let Some(path) = uri_to_path(uri) else {
            return SemanticTokensResult::Tokens(SemanticTokens::default());
        };
        let n = self.legend_tags.len() as u32;
        let mut raw: Vec<(u32, u32, u32, u32)> = Vec::new(); // line, start, len, type
        for tag in self.index.get(&path) {
            // Map from the current tag list to the legend, which was fixed at startup.
            let Some(legend_idx) = self.legend_tags.iter().position(|t| *t == self.scanner.tags()[tag.kind]) else {
                continue;
            };
            raw.push((tag.line, tag.start, tag.len, legend_idx as u32));
            if tag.text_len > 0 {
                raw.push((tag.line, tag.text_start, tag.text_len, n + legend_idx as u32));
            }
        }
        raw.sort();

        let mut data = Vec::with_capacity(raw.len());
        let (mut prev_line, mut prev_start) = (0, 0);
        for (line, start, length, token_type) in raw {
            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 { start - prev_start } else { start };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type,
                token_modifiers_bitset: 0,
            });
            prev_line = line;
            prev_start = start;
        }
        SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data })
    }

    fn symbol_for(&self, path: &Path, tag: &Tag, with_container: bool) -> Option<SymbolInformation> {
        let tag_name = self.scanner.tags().get(tag.kind)?;
        let name = if tag.text.is_empty() {
            tag_name.clone()
        } else {
            format!("{tag_name}: {}", tag.text)
        };
        let container_name = with_container
            .then(|| format!("{}:{}", relative_path(path, &self.roots).display(), tag.line + 1));
        #[allow(deprecated)]
        Some(SymbolInformation {
            name,
            kind: symbol_kind(tag_name),
            tags: None,
            deprecated: None,
            location: Location {
                uri: path_to_uri(path)?,
                range: tag_range(tag),
            },
            container_name,
        })
    }

    fn workspace_symbols(&self, query: &str) -> WorkspaceSymbolResponse {
        let hits = self.index.search(query, self.scanner.tags(), &self.roots, SYMBOL_LIMIT);
        WorkspaceSymbolResponse::Flat(
            hits.iter()
                .filter_map(|hit| self.symbol_for(hit.path, hit.tag, true))
                .collect(),
        )
    }

    fn document_symbols(&self, uri: &Uri) -> DocumentSymbolResponse {
        let Some(path) = uri_to_path(uri) else {
            return DocumentSymbolResponse::Flat(vec![]);
        };
        DocumentSymbolResponse::Flat(
            self.index
                .get(&path)
                .iter()
                .filter_map(|tag| self.symbol_for(&path, tag, false))
                .collect(),
        )
    }

    fn publish_diagnostics(&mut self, path: &Path) -> Result<()> {
        if !self.config.diagnostics {
            return Ok(());
        }
        let Some(uri) = path_to_uri(path) else { return Ok(()) };
        let diagnostics: Vec<Diagnostic> = self
            .index
            .get(path)
            .iter()
            .map(|tag| {
                let tag_name = &self.scanner.tags()[tag.kind];
                Diagnostic {
                    range: tag_range(tag),
                    severity: Some(DiagnosticSeverity::HINT),
                    source: Some("todo-lens".into()),
                    code: Some(NumberOrString::String(tag_name.clone())),
                    message: if tag.text.is_empty() {
                        tag_name.clone()
                    } else {
                        format!("{tag_name}: {}", tag.text)
                    },
                    ..Default::default()
                }
            })
            .collect();
        if diagnostics.is_empty() {
            self.diagnosed.retain(|p| p != path);
        } else if !self.diagnosed.iter().any(|p| p == path) {
            self.diagnosed.push(path.to_path_buf());
        }
        self.notify::<notif::PublishDiagnostics>(PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: None,
        })
    }

    fn publish_all_diagnostics(&mut self) -> Result<()> {
        if !self.config.diagnostics {
            return Ok(());
        }
        // Clear files that no longer have tags, then publish the current ones.
        let stale: Vec<PathBuf> = self.diagnosed.clone();
        let current: Vec<PathBuf> = self.index.files().map(|(p, _)| p.clone()).collect();
        for path in stale.iter().chain(current.iter()) {
            self.publish_diagnostics(path)?;
        }
        Ok(())
    }

    fn clear_diagnostics(&mut self) -> Result<()> {
        for path in std::mem::take(&mut self.diagnosed) {
            if let Some(uri) = path_to_uri(&path) {
                self.notify::<notif::PublishDiagnostics>(PublishDiagnosticsParams {
                    uri,
                    diagnostics: vec![],
                    version: None,
                })?;
            }
        }
        Ok(())
    }

    fn register_file_watcher(&mut self) -> Result<()> {
        if !self.supports_watching {
            return Ok(());
        }
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/*".into()),
                kind: None,
            }],
        };
        self.request::<req::RegisterCapability>(RegistrationParams {
            registrations: vec![Registration {
                id: "todo-lens-watcher".into(),
                method: notif::DidChangeWatchedFiles::METHOD.into(),
                register_options: Some(serde_json::to_value(options)?),
            }],
        })
    }

    fn refresh_tokens(&mut self) -> Result<()> {
        if self.supports_token_refresh {
            self.request::<req::SemanticTokensRefresh>(())?;
        }
        Ok(())
    }

    fn show_message(&self, typ: MessageType, message: &str) -> Result<()> {
        self.notify::<notif::ShowMessage>(ShowMessageParams {
            typ,
            message: message.into(),
        })
    }

    fn notify<N: notif::Notification>(&self, params: N::Params) -> Result<()> {
        let n = Notification::new(N::METHOD.into(), params);
        self.connection.sender.send(Message::Notification(n))?;
        Ok(())
    }

    fn request<R: req::Request>(&mut self, params: R::Params) -> Result<()> {
        self.next_request_id += 1;
        let id = RequestId::from(format!("todo-lens-{}", self.next_request_id));
        let r = Request::new(id, R::METHOD.into(), params);
        self.connection.sender.send(Message::Request(r))?;
        Ok(())
    }
}

/// Icons in the symbol pickers. Only the icon changes; the kind has no meaning here.
fn symbol_kind(tag: &str) -> SymbolKind {
    match tag {
        "TODO" => SymbolKind::EVENT,
        "BUG" => SymbolKind::OPERATOR,
        "FIXME" => SymbolKind::KEY,
        "NOTE" => SymbolKind::STRING,
        _ => SymbolKind::CONSTANT,
    }
}

fn tag_range(tag: &Tag) -> Range {
    let end = if tag.text_len > 0 { tag.text_start + tag.text_len } else { tag.start + tag.len };
    Range::new(Position::new(tag.line, tag.start), Position::new(tag.line, end))
}

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

fn path_to_uri(path: &Path) -> Option<Uri> {
    let url = url::Url::from_file_path(path).ok()?;
    Uri::from_str(url.as_str()).ok()
}
