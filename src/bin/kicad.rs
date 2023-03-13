use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use async_trait::async_trait;
use futures_lite::StreamExt;
use merge::Merge;
use pop_launcher::{async_stdin, json_input_stream, PluginResponse, PluginSearchResult};

use log::{error, info, warn, LevelFilter};
use serde::Deserialize;

use pop_launcher_plugins::*;

#[derive(Deserialize, Merge, Default)]
struct Config {
    path: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    systemd_journal_logger::init().unwrap();
    log::set_max_level(LevelFilter::Info);
    info!("Loaded pop launcher kicad integration");

    let config = if let Ok(config_files) = get_config_files("kicad") {
        get_config(&config_files)
    } else {
        Config::default()
    };

    let path_option = match config.path {
        Some(path) if PathBuf::from(&path).exists() => Some(path),
        _ => {
            warn!("Falling back to homedir");
            home::home_dir().and_then(|path| path.to_str().map(|x| x.to_owned()))
        }
    };

    let path = match path_option {
        Some(path) => path,
        None => {
            error!("Could not find configured or home directory");
            return;
        }
    };

    let mut requests = json_input_stream(async_stdin());

    let mut plugin = KicadPlugin::new(&path).unwrap();

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

#[derive(Debug)]
struct KicadProject {
    path: PathBuf,
    name: String,
}

struct KicadPlugin {
    projects: Vec<KicadProject>,
    matcher: fuzzy_matcher::skim::SkimMatcherV2,
    responder: Responder,
}

#[async_trait(?Send)]
impl PopLauncherPlugin for KicadPlugin {
    async fn search(&mut self, pat: &str) {
        let query = match pat.strip_prefix("kicad ") {
            Some(pat) => pat,
            None => {
                warn!("Search query did not match: {pat}");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        self.responder.respond(PluginResponse::Clear).await;

        info!("Starting search with pattern: {query}");

        for (id, project) in self
            .projects
            .iter()
            .enumerate()
            .filter(|(_id, project)| self.matcher.fuzzy(&project.name, query, false).is_some())
        {
            self.responder
                .respond(PluginResponse::Append(PluginSearchResult {
                    id: id as u32,
                    name: project.name.clone(),
                    description: "".to_owned(),
                    keywords: None,
                    icon: Some(pop_launcher::IconSource::Name(Cow::Borrowed("kicad"))),
                    exec: None,
                    window: None,
                }))
                .await;
        }

        self.responder.respond(PluginResponse::Finished).await;
    }

    async fn activate(&mut self, id: u32) {
        let item = match self.projects.get(id as usize) {
            Some(item) => item,
            None => return,
        };

        info!("Activating {item:?}");
        if tokio::process::Command::new("kicad")
            .arg(item.path.clone())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_err()
        {
            error!("Could not open project {}", item.name);
        }

        self.responder.respond(PluginResponse::Close).await;
    }
}

impl KicadPlugin {
    fn new(path: &str) -> anyhow::Result<Self> {
        Ok(Self {
            projects: get_projects(path),
            responder: Responder::default(),
            matcher: fuzzy_matcher::skim::SkimMatcherV2::default(),
        })
    }
}

fn get_projects(search: &str) -> Vec<KicadProject> {
    let output = Command::new("fd")
        .arg("kicad_pro")
        .arg(search)
        .output()
        .unwrap();
    let paths = String::from_utf8(output.stdout).unwrap();

    paths
        .split_terminator('\n')
        .map(|path| {
            let path = PathBuf::from(&path);
            let name = get_project_name(&path).unwrap();
            KicadProject { path, name }
        })
        .collect()
}

fn get_project_name(path: &Path) -> Option<String> {
    Some(String::from(path.file_stem()?.to_str()?))
}
