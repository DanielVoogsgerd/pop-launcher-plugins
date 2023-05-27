use std::{borrow::Cow, fmt::Display};

use async_trait::async_trait;
use futures_lite::StreamExt;
use log::{error, info, warn, LevelFilter};
use pop_launcher::{async_stdin, json_input_stream, PluginResponse, PluginSearchResult};
use pop_launcher_plugins::{PopLauncherPlugin, Responder};

const PLUGIN_PREFIX: &str = "media";

#[derive(PartialEq, Eq, Debug)]
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

impl TryFrom<&'static str> for PlayerControls {
    type Error = &'static str;

    fn try_from(value: &'static str) -> Result<Self, Self::Error> {
        for control in PlayerControls::iter() {
            let control_str: &str = (&control).into();
            if value == control_str {
                return Ok(control);
            }
        }

        Err("Could not find matching control")
    }
}

impl Display for &PlayerControls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str((*self).into())
    }
}

impl PlayerControls {
    fn iter() -> impl Iterator<Item = PlayerControls> {
        [
            PlayerControls::VolumeUp,
            PlayerControls::VolumeDown,
            PlayerControls::Play,
            PlayerControls::Pause,
        ]
        .into_iter()
    }

    fn get_matches(matcher: &fuzzy_matcher::skim::SkimMatcherV2, query: &str) -> Vec<Self> {
        Self::iter()
            .filter(move |action| {
                let action_str: &str = action.into();
                matcher.fuzzy(action_str, query, false).is_some()
            })
            .collect()
    }
}

#[derive(Debug)]
enum Item {
    Player(mpris::Player),
    Action(mpris::Player, PlayerControls),
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut requests = json_input_stream(async_stdin());

    systemd::journal::JournalLog::init().unwrap();
    log::set_max_level(LevelFilter::Info);
    info!("Loaded pop launcher MPRIS integration");

    let mut plugin = MprisPlugin::new().unwrap();

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

    warn!("Stopping");
}

struct MprisPlugin {
    mpris: mpris::PlayerFinder,
    responder: Responder,
    matcher: fuzzy_matcher::skim::SkimMatcherV2,
    items: Vec<Item>,
}

impl MprisPlugin {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            mpris: mpris::PlayerFinder::new()?,
            responder: Responder::default(),
            matcher: fuzzy_matcher::skim::SkimMatcherV2::default(),
            items: Vec::new(),
        })
    }

    fn format_player(&self, player: &mpris::Player) -> Option<PluginSearchResult> {
        Some(PluginSearchResult {
            id: self.items.len() as u32,
            name: player.identity().to_owned(),
            description: String::from("Description"),
            keywords: None,
            icon: get_player_icon(player),
            exec: None,
            window: None,
        })
    }

    fn format_action(&self, action: &PlayerControls) -> Option<PluginSearchResult> {
        let name: &str = action.into();
        Some(PluginSearchResult {
            id: self.items.len() as u32,
            name: name.to_owned(),
            description: String::new(),
            keywords: None,
            icon: get_action_icon(action),
            exec: None,
            window: None,
        })
    }

    async fn clear(&mut self) {
        self.items.clear();
        self.responder.respond(PluginResponse::Clear).await;
    }

    async fn add_player(&mut self, item: mpris::Player) {
        match self.format_player(&item) {
            Some(item) => self.responder.respond(PluginResponse::Append(item)).await,
            None => return,
        };
        self.items.push(Item::Player(item));
    }

    async fn add_action(&mut self, player: mpris::Player, action: PlayerControls) {
        match self.format_action(&action) {
            Some(item) => self.responder.respond(PluginResponse::Append(item)).await,
            None => return,
        };
        self.items.push(Item::Action(player, action));
    }

    fn get_player_matches(&mut self, query: &str) -> Vec<mpris::Player> {
        self.get_all_players()
            .into_iter()
            .filter(move |player| {
                self.matcher
                    .fuzzy(player.identity(), query, false)
                    .is_some()
            })
            .collect()
    }

    fn get_all_players(&mut self) -> Vec<mpris::Player> {
        match self.mpris.find_all() {
            Ok(players) => players,
            Err(_) => {
                warn!("Could not find players");
                Vec::new()
            }
        }
    }
}

#[async_trait(?Send)]
impl PopLauncherPlugin for MprisPlugin {
    async fn search(&mut self, input: &str) {
        info!("Searching for players");

        self.clear().await;

        let input = match input.strip_prefix(&format!("{PLUGIN_PREFIX} ")) {
            Some(pat) => pat,
            None => {
                warn!("Search query did not match: {input}");
                self.responder.respond(PluginResponse::Finished).await;
                return;
            }
        };

        let player_option = self
            .get_all_players()
            .into_iter()
            .find(|player| input.starts_with(player.identity()));

        match player_option {
            Some(player) => {
                let input = input.strip_prefix(player.identity()).unwrap().trim_start();

                for action in PlayerControls::get_matches(&self.matcher, input) {
                    // Basically dereferencing it, but it does not implement copy nor clone
                    let player = match self.mpris.find_by_name(player.identity()) {
                        Ok(x) => x,
                        Err(_) => {
                            warn!("Could not find relevant player");
                            self.responder.respond(PluginResponse::Finished).await;
                            return;
                        }
                    };

                    self.add_action(player, action).await;
                }
            }
            None => {
                for player in self.get_player_matches(input) {
                    self.add_player(player).await;
                }
            }
        }

        self.responder.respond(PluginResponse::Finished).await;
    }

    async fn activate(&mut self, id: u32) {
        let item = match self.items.get(id as usize) {
            Some(item) => item,
            None => {
                error!("Could not activate item with id {id}");
                return;
            }
        };

        if match item {
            Item::Player(_player) => {
                self.complete(id).await;
                anyhow::Result::Ok(())
            }
            Item::Action(player, action) => match action {
                PlayerControls::VolumeUp => increase_volume(player, 0.1),
                PlayerControls::VolumeDown => decrease_volume(player, 0.1),
                PlayerControls::Play => {
                    if play(player).is_err() {
                        error!("Could not play");
                    };
                    self.responder.respond(PluginResponse::Close).await;
                    anyhow::Result::Ok(())
                }
                PlayerControls::Pause => {
                    if pause(player).is_err() {
                        error!("Could not pause");
                    };
                    self.responder.respond(PluginResponse::Close).await;
                    anyhow::Result::Ok(())
                }
            },
        }
        .is_err()
        {
            error!("Could not activate item with id {id}");
        }
    }

    async fn complete(&mut self, id: u32) {
        let input = match self.items.get(id as usize) {
            Some(Item::Player(player)) => format!("{PLUGIN_PREFIX} {} ", player.identity()),
            Some(Item::Action(player, action)) => {
                format!("{PLUGIN_PREFIX} {} {}", player.identity(), action)
            }
            None => {
                warn!("Item does not exist");
                return;
            }
        };
        self.responder
            .respond(PluginResponse::Fill(input.clone()))
            .await;
        self.search(&input).await;
    }
}

fn increase_volume(player: &mpris::Player, increase: f64) -> anyhow::Result<()> {
    let current_volume = player.get_volume()?;
    player.set_volume(current_volume + increase)?;
    Ok(())
}

fn decrease_volume(player: &mpris::Player, decrease: f64) -> anyhow::Result<()> {
    let current_volume = player.get_volume()?;
    if decrease > current_volume {
        player.set_volume(0f64)?;
    } else {
        player.set_volume(current_volume - decrease)?;
    }
    Ok(())
}

fn play(player: &mpris::Player) -> anyhow::Result<()> {
    player.play()?;
    Ok(())
}

fn pause(player: &mpris::Player) -> anyhow::Result<()> {
    player.pause()?;
    Ok(())
}

fn get_player_icon(player: &mpris::Player) -> Option<pop_launcher::IconSource> {
    match player.identity() {
        "Spotify" => Some(pop_launcher::IconSource::Name(Cow::Borrowed("spotify"))),
        "Mozilla Firefox" => Some(pop_launcher::IconSource::Name(Cow::Borrowed("firefox"))),
        _ => Some(pop_launcher::IconSource::Name(Cow::Borrowed(
            "folder-music",
        ))),
    }
}

fn get_action_icon(action: &PlayerControls) -> Option<pop_launcher::IconSource> {
    match action {
        PlayerControls::VolumeUp => Some(pop_launcher::IconSource::Name(Cow::Borrowed(
            "audio-volume-high",
        ))),
        PlayerControls::VolumeDown => Some(pop_launcher::IconSource::Name(Cow::Borrowed(
            "audio-volume-low",
        ))),
        PlayerControls::Play => Some(pop_launcher::IconSource::Name(Cow::Borrowed("player_play"))),
        PlayerControls::Pause => Some(pop_launcher::IconSource::Name(Cow::Borrowed(
            "player_pause",
        ))),
    }
}
