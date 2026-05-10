mod backend;
mod document;
mod features;
mod language;
mod store;
mod symbols;

use backend::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let lang = language::gams_language();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) =
        LspService::new(move |client| Backend::new(client, lang.clone()));
    Server::new(stdin, stdout, socket).serve(service).await;
}
