use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::anyhow;
use async_trait::async_trait;
use futures_lite::StreamExt;
use itertools::Itertools;
use log::{error, info, warn, LevelFilter};
use pop_launcher::{async_stdin, json_input_stream, PluginResponse, PluginSearchResult};
use pop_launcher_plugins::PopLauncherPlugin;

const PLUGIN_PREFIX: &str = "rust";

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    r#type: Type,
    file_path: PathBuf,
}

#[derive(Debug, Clone)]
enum Type {
    Constant,
    Enum,
    Function,
    Keyword,
    Macro,
    Module,
    Primitive,
    Struct,
    Trait,
    Type,
}

impl TryFrom<&str> for Type {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "fn" => Ok(Type::Function),
            "struct" => Ok(Type::Struct),
            "constant" => Ok(Type::Constant),
            "trait" => Ok(Type::Trait),
            "macro" => Ok(Type::Macro),
            "type" => Ok(Type::Type),
            "enum" => Ok(Type::Enum),
            "primitive" => Ok(Type::Primitive),
            "keyword" => Ok(Type::Keyword),
            _ => Err(format!("Could not find type {value}")),
        }
    }
}

impl From<Type> for String {
    fn from(val: Type) -> Self {
        match val {
            Type::Function => "function",
            Type::Struct => "struct",
            Type::Keyword => "keyword",
            Type::Macro => "macro",
            Type::Trait => "trait",
            Type::Module => "module",
            Type::Primitive => "primitive",
            Type::Constant => "contant",
            Type::Enum => "enum",
            Type::Type => "type",
        }
        .to_owned()
    }
}

#[derive(Default)]
struct RustDocsPlugin {
    items: Vec<Entry>,
    responder: pop_launcher_plugins::Responder,
    matcher: fuzzy_matcher::skim::SkimMatcherV2,
}

#[async_trait(?Send)]
impl PopLauncherPlugin for RustDocsPlugin {
    async fn search(&mut self, input: &str) {
        let input = match input.strip_prefix(&format!("{PLUGIN_PREFIX} ")) {
            Some(input) => input,
            None => {
                warn!("Search query did not match: {input}");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        self.responder.respond(PluginResponse::Clear).await;
        self.items.clear();
        info!("Starting search with pattern: {input}");

        let mut segments = input.split("::").collect::<Vec<_>>();
        let search_term = segments.pop().unwrap();

        let index_path = match get_index_path() {
            Ok(index_path) => index_path,
            _ => {
                error!("Could not find index path; aborting search");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        let mut path = PathBuf::from(&index_path);
        path.pop();
        let path = path.join(segments.join("/"));

        if !path.exists() {
            warn!("Index path does not exist; aborting search: {path:?}");
            self.responder.respond(PluginResponse::Finished).await;
            return;
        }

        for (id, entry) in path
            .read_dir()
            .unwrap()
            .filter_map(|x| x.ok())
            .filter_map(|dir_entry| {
                let filename = dir_entry.file_name();

                if dir_entry.file_type().ok()?.is_dir() {
                    return Some(Entry {
                        name: filename.to_str()?.to_owned(),
                        r#type: Type::Module,
                        file_path: dir_entry.path().join("index.html"),
                    });
                }

                let mut file_segments = filename.to_str().unwrap().split('.');

                let item_type = file_segments.next()?;
                let name = file_segments.next()?;

                Some(Entry {
                    name: name.to_owned(),
                    r#type: item_type.try_into().ok()?,
                    file_path: dir_entry.path(),
                })
            })
            .filter_map(|x| {
                if let Some(score) = self.matcher.fuzzy(&x.name, search_term, false) {
                    Some((x, score.0))
                } else {
                    None
                }
            })
            .sorted_by(|(_x, x_score), (_y, y_score)| y_score.cmp(x_score))
            .take(10)
            .map(|(entry, _score)| entry)
            .enumerate()
        {
            self.items.push(entry.clone());
            self.responder
                .respond(PluginResponse::Append(PluginSearchResult {
                    id: id as u32,
                    name: entry.name,
                    description: entry.r#type.into(),
                    keywords: None,
                    icon: None,
                    exec: None,
                    window: None,
                }))
                .await;
        }

        self.responder.respond(PluginResponse::Finished).await;
    }

    async fn activate(&mut self, id: u32) {
        let item = match self.items.get(id as usize) {
            Some(item) => item,
            None => {
                warn!("Could not activate item with id {id}");
                return;
            }
        };

        info!("Activating {item:?}");
        if tokio::process::Command::new("xdg-open")
            .arg(item.file_path.clone())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_err()
        {
            error!("Could not open docs {}", item.name);
        }

        self.responder.respond(PluginResponse::Close).await;
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    systemd_journal_logger::init().unwrap();
    log::set_max_level(LevelFilter::Info);
    info!("Loaded pop launcher rust docs integration");

    let mut requests = json_input_stream(async_stdin());

    let mut plugin = RustDocsPlugin::default();

    while let Some(request_res) = requests.next().await {
        let request = match request_res {
            Ok(x) => x,
            Err(_error) => {
                warn!("Error occured when retrieving requests.");
                continue;
            }
        };

        plugin.request(request).await;
    }
}

fn get_index_path() -> anyhow::Result<String> {
    let index_path = String::from_utf8(
        Command::new("rustup")
            .arg("doc")
            .arg("--path")
            .output()?
            .stdout,
    )?;

    Ok(index_path
        .strip_suffix('\n')
        .ok_or(anyhow!("Could not strip newline from index path"))?
        .to_owned())
}
