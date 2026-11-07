// No console window behind the app; debug builds keep it for their messages.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod analysis;
mod assets;
mod catalog;
mod collections;
mod config;
mod cover_host;
mod decoder;
mod discord;
mod download;
mod dsp;
mod engine;
mod http_source;
mod library;
mod lyrics;
mod memory;
mod online_cover;
mod quality;
mod queue;
mod sources;
mod telegram;
mod theme;
mod ui;
mod update;
mod visuals;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use gpui::{App, Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

use sources::subsonic::{Credentials, Subsonic};
use ui::Startup;

const USAGE: &str = "Utilisation :
  onde                                  ouvre l'application
  onde DOSSIER                          ouvre un dossier de musique
  onde navidrome                        ouvre le serveur configuré
  onde navidrome-login ADRESSE UTILISATEUR
  onde navidrome-logout
  onde --update                         installe la dernière version
  onde --version";

fn navidrome_login(url: &str, user: &str) -> Result<()> {
    let password = rpassword::prompt_password("Mot de passe : ")?;
    let credentials = Credentials::new(url, user, &password)?;
    Subsonic::new(credentials.clone()).ping()?;
    let plain_http = credentials.url.starts_with("http://");
    let mut settings = config::load();
    settings.navidrome = Some(credentials);
    let path = config::save(&settings)?;
    println!("Connecté. Identifiants enregistrés dans {}", path.display());
    println!("Le mot de passe n'est pas conservé, seulement son empreinte salée.");
    if plain_http {
        println!("Attention : en http, cette empreinte circule en clair sur le réseau.");
    }
    Ok(())
}

fn navidrome_logout() -> Result<()> {
    let mut settings = config::load();
    settings.navidrome = None;
    config::save(&settings)?;
    println!("Identifiants Navidrome supprimés.");
    Ok(())
}

fn self_update() -> Result<()> {
    let Some(found) = update::check()? else {
        println!("Onde {} est à jour.", update::CURRENT);
        return Ok(());
    };
    println!("Installation d'Onde {}…", found.version);
    update::install(&found)?;
    println!(
        "Onde {} est installé. Il prendra effet au prochain lancement.",
        found.version
    );
    Ok(())
}

fn run_command(args: &[String]) -> Result<()> {
    match args {
        [command] if command == "--update" => self_update(),
        [command] if command == "--version" => {
            println!("onde {}", update::CURRENT);
            Ok(())
        }
        [_, url, user] if args[0] == "navidrome-login" => navidrome_login(url, user),
        [command] if command == "navidrome-logout" => navidrome_logout(),
        _ => bail!("{USAGE}"),
    }
}

fn main() -> ExitCode {
    memory::limit_pools();
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let startup = match args.first().map(String::as_str) {
        None => Startup::Empty,
        Some("navidrome") => Startup::Navidrome,
        Some(
            "navidrome-login" | "navidrome-logout" | "--help" | "-h" | "--update" | "--version",
        ) => {
            return match run_command(&args) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Some(_) => Startup::Folder(PathBuf::from(
            std::env::args_os().nth(1).unwrap_or_default(),
        )),
    };
    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            if let Err(error) = assets::load_fonts(cx) {
                eprintln!("Polices non chargées : {error:#}");
            }
            ui::bind_keys(cx);
            let bounds = Bounds::centered(None, size(px(1180.), px(780.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    app_id: Some("onde".into()),
                    ..Default::default()
                },
                move |window, cx| cx.new(|cx| ui::Onde::new(startup, window, cx)),
            )
            .expect("ouverture de la fenêtre");
            cx.on_window_closed(|cx| cx.quit()).detach();
            cx.activate(true);
        });
    ExitCode::SUCCESS
}
