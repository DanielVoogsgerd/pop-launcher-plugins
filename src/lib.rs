use async_trait::async_trait;
use futures_lite::AsyncWriteExt;
use log::warn;
use pop_launcher::{async_stdout, PluginResponse, Request};

pub struct Responder {
    output: blocking::Unblock<std::io::Stdout>,
}

impl Responder {
    pub async fn respond(&mut self, response: PluginResponse) {
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

impl Default for Responder {
    fn default() -> Self {
        Self {
            output: async_stdout(),
        }
    }
}

#[async_trait(?Send)]
pub trait PopLauncherPlugin {
    // Required
    async fn search(&mut self, query: &str);
    async fn activate(&mut self, id: u32);

    // Optional
    async fn activate_context(&self, _id: u32, _context: u32) {}
    async fn complete(&mut self, _id: u32) {}
    async fn context(&self, _id: u32) {}
    async fn exit(&self) {}
    async fn interrupt(&self) {}
    async fn quit(&self, _id: u32) {}

    async fn request(&mut self, req: Request) {
        match req {
            Request::Search(input) => self.search(&input).await,
            Request::Activate(id) => self.activate(id).await,

            Request::ActivateContext { id, context } => self.activate_context(id, context).await,
            Request::Complete(id) => self.complete(id).await,
            Request::Context(id) => self.context(id).await,
            Request::Exit => self.exit().await,
            Request::Interrupt => self.interrupt().await,
            Request::Quit(id) => self.quit(id).await,
        }
    }
}
