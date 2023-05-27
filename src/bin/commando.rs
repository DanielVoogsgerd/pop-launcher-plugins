use std::{
    borrow::Cow,
    fs::{read_dir, read_to_string},
    iter::once,
    process::Stdio,
};

use async_trait::async_trait;
use futures_lite::StreamExt;
use pop_launcher::{async_stdin, json_input_stream, PluginResponse, PluginSearchResult};

use log::{error, info, warn, LevelFilter};
use pop_launcher_plugins::*;
use serde::Deserialize;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    systemd::journal::JournalLog::init().unwrap();
    log::set_max_level(LevelFilter::Info);
    info!("Loaded pop launcher Commando plugin");

    let mut requests = json_input_stream(async_stdin());

    let mut plugin = CommandoPlugin::new().unwrap();

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

#[derive(Deserialize)]
struct CommandFile {
    commands: Vec<Command>,
}

#[derive(Debug, Deserialize)]
struct Command {
    name: String,
    command: String,
    icon: Option<String>,
}

struct CommandoPlugin {
    commands: Vec<Command>,
    matcher: fuzzy_matcher::skim::SkimMatcherV2,
    responder: Responder,
}

impl CommandoPlugin {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            commands: get_commands().unwrap_or_default(),
            responder: Responder::default(),
            matcher: fuzzy_matcher::skim::SkimMatcherV2::default(),
        })
    }
}

#[async_trait(?Send)]
impl PopLauncherPlugin for CommandoPlugin {
    async fn search(&mut self, query: &str) {
        self.responder.respond(PluginResponse::Clear).await;

        info!("Starting search with pattern: {query}");

        for (id, command) in self
            .commands
            .iter()
            .enumerate()
            .filter(|(_id, command)| self.matcher.fuzzy(&command.name, query, false).is_some())
        {
            self.responder
                .respond(PluginResponse::Append(PluginSearchResult {
                    id: id as u32,
                    name: command.name.clone(),
                    description: "".to_owned(),
                    keywords: None,
                    icon: command
                        .icon
                        .clone()
                        .map(|icon_str| (pop_launcher::IconSource::Name(Cow::Owned(icon_str)))),
                    exec: None,
                    window: None,
                }))
                .await;
        }

        self.responder.respond(PluginResponse::Finished).await;
    }

    async fn activate(&mut self, id: u32) {
        let item = match self.commands.get(id as usize) {
            Some(item) => item,
            None => return,
        };

        info!("Activating {item:?}");

        let split_command = match shlex::split(&item.command) {
            Some(split_command) => split_command,
            _ => {
                error!("Could not split command");
                self.responder.respond(PluginResponse::Close).await;
                return;
            }
        };

        let mut command_iter = split_command.into_iter();
        let command_string = match command_iter.next() {
            Some(command_string) => command_string,
            _ => {
                error!("Could not split command");
                self.responder.respond(PluginResponse::Close).await;
                return;
            }
        };

        if tokio::process::Command::new(command_string)
            .args(command_iter)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_err()
        {
            error!("Could not run command {}", item.command);
        }

        self.responder.respond(PluginResponse::Close).await;
    }
}

fn get_command_files() -> anyhow::Result<Vec<CommandFile>> {
    let xdg = xdg::BaseDirectories::with_prefix("commando")?;
    let home = xdg.get_config_home();
    let dirs = xdg.get_config_dirs();

    Ok(dirs
        .into_iter()
        .chain(once(home))
        .map(|dir| dir.join("commandos"))
        .filter_map(|command_dir| {
            let files = read_dir(command_dir).ok()?;
            Some(files.filter_map(|file| Some(file.ok()?.path())))
        })
        .flatten()
        .filter_map(|file| toml::from_str(&read_to_string(file).ok()?).ok())
        .collect::<Vec<_>>())
}

fn get_commands() -> anyhow::Result<Vec<Command>> {
    Ok(get_command_files()?
        .into_iter()
        .flat_map(|file| file.commands)
        .collect())
}
