use std::{ffi::OsStr, path::Path};

use futures_lite::{AsyncWriteExt, StreamExt};
use pop_launcher::{
    async_stdin, async_stdout, json_input_stream, PluginResponse, PluginSearchResult, Request,
};

use log::{info, warn, LevelFilter};

extern crate notmuch;

struct Responder {
    output: blocking::Unblock<std::io::Stdout>,
}

pub fn xdg_open<S: AsRef<OsStr>>(file: S) {
    let _ = tokio::process::Command::new("xdg-open").arg(file).spawn();
}

impl Responder {
    fn new() -> Self {
        Self {
            output: async_stdout(),
        }
    }

    async fn respond(&mut self, response: PluginResponse) -> () {
        let mut data = match serde_json::to_string(&response) {
            Ok(data) => data,
            Err(_) => {
                warn!("Could not serialize response as json");
                return;
            }
        };
        data.push('\n');

        if self.output.write_all(data.as_bytes()).await.is_err() {
            warn!("Could not write output");
            return;
        }

        if self.output.flush().await.is_err() {
            warn!("Could not flush output");
        };
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut requests = json_input_stream(async_stdin());

    systemd_journal_logger::init().unwrap();
    log::set_max_level(LevelFilter::Info);
    info!("Loaded pop launcher notmuch integration");

    let database = notmuch::Database::open_with_config(
        Some(Path::new("/home/daniel/mail/")),
        notmuch::DatabaseMode::ReadOnly,
        Some(Path::new("/home/daniel/.config/notmuch/notmuchrc")),
        None,
    )
    .unwrap();
    info!("Loaded notmuch database");

    let mut plugin = NotmuchPlugin::new(database);

    while let Some(request_res) = requests.next().await {
        let request = match request_res {
            Ok(x) => x,
            Err(_error) => {
                warn!("Error occured when retrieving requests.");
                continue;
            }
        };
        match request {
            Request::Search(input) => plugin.search(input).await,
            Request::Activate(id) => {
                let item = match plugin.items.get(id as usize) {
                    Some(item) => item,
                    None => continue,
                };
                let id = item.id();
                info!("Received activate request");
                xdg_open(format!("notmuch://thread/{id}"));
                plugin.responder.respond(PluginResponse::Close).await;
            }
            Request::ActivateContext { id, context } => {
                warn!("Ignoring activate context request");
            }
            Request::Complete(_) => {
                warn!("Ignoring complete request");
            }
            Request::Context(_) => {
                warn!("Ignoring context request");
            }
            Request::Exit => {
                warn!("Ignoring exit request");
            }
            Request::Interrupt => {
                warn!("Ignoring interupt request");
            }
            Request::Quit(_) => {
                warn!("Ignoring quit request");
            }
        }
    }

    warn!("Stopping");
}

struct NotmuchPlugin {
    database: notmuch::Database,
    responder: Responder,
    items: Vec<notmuch::Thread>,
}

impl NotmuchPlugin {
    fn new(database: notmuch::Database) -> Self {
        Self {
            database,
            responder: Responder::new(),
            items: Vec::new(),
        }
    }

    async fn search(&mut self, input: String) {
        let query = match input.strip_prefix("notmuch ") {
            Some(cap) => cap,
            None => {
                warn!("Search query did not match: {input}");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };
        info!("Received request with query {query}");
        let query = match self.database.create_query(&query) {
            Ok(query) => query,
            Err(_) => {
                warn!("Could not query notmuch database");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        let threads = match query.search_threads() {
            Ok(threads) => threads,
            Err(_) => {
                warn!("Could not search threads");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        self.clear().await;
        for thread in threads.into_iter().take(20) {
            self.add_item(thread).await;
        }

        self.responder.respond(PluginResponse::Finished).await;
    }

    fn format_item(&self, item: &notmuch::Thread) -> Option<PluginSearchResult> {
        Some(PluginSearchResult {
            id: self.items.len() as u32,
            name: item.subject().into_owned(),
            description: String::from("Description"),
            keywords: None,
            icon: None,
            exec: None,
            window: None,
        })
    }

    async fn clear(&mut self) {
        self.items.clear();
        self.responder.respond(PluginResponse::Clear).await;
    }

    async fn add_item(&mut self, item: notmuch::Thread) {
        match self.format_item(&item) {
            Some(item) => self.responder.respond(PluginResponse::Append(item)).await,
            None => return,
        };
        self.items.push(item);
    }
}
