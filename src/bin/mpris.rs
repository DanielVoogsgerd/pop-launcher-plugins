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

#[derive(PartialEq, Eq)]
enum PlayerControls {
    VolumeUp,
    VolumeDown,
    Play,
    Pause,
}

impl From<&PlayerControls> for &'static str {
    fn from(val: &PlayerControls) -> Self {
        match val {
            PlayerControls::VolumeUp => "Volume up",
            PlayerControls::VolumeDown => "Volume down",
            PlayerControls::Play => "Play",
            PlayerControls::Pause => "Pause",
        }
    }
}

impl PlayerControls {
    fn iter() -> impl Iterator<Item = PlayerControls> {
        [
            PlayerControls::VolumeUp,
            PlayerControls::VolumeDown,
            PlayerControls::Play,
            PlayerControls::Pause,
        ].into_iter()
    }
}

impl TryFrom<&'static str> for PlayerControls {
    type Error = &'static str;

    fn try_from(value: &'static str) -> Result<Self, Self::Error> {
        for control in PlayerControls::iter() {
            let control_str: &str = (&control).into();
            if value == control_str {
                return Ok(control);
            }
        }

        return Err("Could not find matching control");
    }
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

    let mut plugin = MprisPlugin::new().unwrap();

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
                // let item = match plugin.items.get(id as usize) {
                //     Some(item) => item,
                //     None => continue,
                // };
                // let id = item.id();
                // info!("Received activate request");
                // xdg_open(format!("notmuch://thread/{id}"));
                // plugin.responder.respond(PluginResponse::Close).await;
            }
            Request::ActivateContext { id, context } => {
                warn!("Ignoring activate context request");
            }
            Request::Complete(id) => {
                let player = match plugin.items.get(id as usize) {
                    Some(item) => item,
                    None => {
                        warn!("Player does not exist");
                        continue;
                    }
                };
                let input = String::from(format!("mpris {}", player.bus_name_player_name_part()));
                plugin
                    .responder
                    .respond(PluginResponse::Fill(input.clone()))
                    .await;
                plugin.search(input).await;
            }
            Request::Context(id) => {
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

struct MprisPlugin {
    mpris: mpris::PlayerFinder,
    responder: Responder,
    items: Vec<mpris::Player>,
}

impl MprisPlugin {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            mpris: mpris::PlayerFinder::new()?,
            responder: Responder::new(),
            items: Vec::new(),
        })
    }

    async fn search(&mut self, input: String) {
        info!("Searching");

        self.clear().await;

        let players = match self.mpris.find_all() {
            Ok(player_name) => player_name,
            Err(_) => {
                warn!("Could not find players");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        let mut segments = input.split_whitespace();

        let mpris_name = segments.next();
        let player_name = segments.next();
        let control_name = segments.next();
        if let Some(control_search) = control_name {
            info!("Attemping to complete control; query: {}", control_search);
            self.clear().await;
            let control_options = ["Volume Up", "Volume Down", "Play", "Pause"];
            for option in control_options {
                if option.starts_with(control_search) {
                    self.responder.respond(PluginResponse::Append(PluginSearchResult {
                        id: 0,
                        name: option.to_owned(),
                        description: "".to_owned(),
                        keywords: None,
                        icon: None,
                        exec: None,
                        window: None,
                    })).await;
                }
            }

            self.responder.respond(PluginResponse::Finished).await;
        } else if let Some(player_search) = player_name {
            for player in players {
                if player
                    .bus_name_player_name_part()
                    .starts_with(player_search)
                {
                    self.add_item(player).await;
                }
            }
        } else {
            for player in players {
                self.add_item(player).await;
            }
        }

        self.responder.respond(PluginResponse::Finished).await;
    }

    fn format_item(&self, item: &mpris::Player) -> Option<PluginSearchResult> {
        Some(PluginSearchResult {
            id: self.items.len() as u32,
            name: item.bus_name_player_name_part().to_owned(),
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

    async fn add_item(&mut self, item: mpris::Player) {
        match self.format_item(&item) {
            Some(item) => self.responder.respond(PluginResponse::Append(item)).await,
            None => return,
        };
        self.items.push(item);
    }
}
