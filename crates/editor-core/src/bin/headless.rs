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
    mcp::serve_stdio(EditorServer::new(application.session.clone(), None))?;
    Ok(())
}
