use editor_core::{
    mcp::{self, EditorServer},
    startup::{Application, Args},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let application = Application::open(&args)?;
    if let Some(Err(message)) = &application.open_message {
        eprintln!("{message}");
    }
    let result = mcp::serve_stdio(EditorServer::for_document(application.document(), None))
        .map_err(Box::<dyn std::error::Error>::from);
    application.finish(result)
}
