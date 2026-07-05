//! Freedback CLI (native). Read / write / sync against a feedback server.
//!
//! The library ([`freedback_cli_client`]) is the dual-target piece; this binary
//! is native-only (filesystem + clap + tokio), gated behind the `native`
//! feature so the crate still builds for `wasm32`.

#[cfg(feature = "native")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    cli::run().await
}

#[cfg(not(feature = "native"))]
fn main() {}

#[cfg(feature = "native")]
mod cli {
    use clap::{Parser, Subcommand};
    use freedback_cli_client::{
        Client, CollectionPoint, Dest, PublicationPoint, ReqwestTransport, Source,
    };
    use freedback_protocol::{Annotation, Body, Identity, Motivation, Target};
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    #[derive(Parser)]
    #[command(name = "freedback", about = "Freedback basic client")]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[derive(Subcommand)]
    enum Cmd {
        /// Sign and publish a rating/comment to a feedback server.
        Write {
            #[arg(long)]
            server: String,
            #[arg(long)]
            target: String,
            #[arg(long)]
            stars: Option<f64>,
            #[arg(long)]
            scalar: Option<f64>,
            #[arg(long)]
            thumb: Option<bool>,
            #[arg(long)]
            comment: Option<String>,
            /// Reuse (or create) a persistent identity instead of a fresh
            /// throwaway one — load the PKCS#8 PEM keypair from this file if
            /// it exists, otherwise generate one and save it here. Lets two
            /// `write` calls act as the SAME issuer (e.g. to supersede an
            /// earlier post for the same target — the only "edit/delete"
            /// this append-only protocol supports; see docs/hosting.md).
            #[arg(long)]
            key_file: Option<std::path::PathBuf>,
        },
        /// Read aggregated feedback for a target.
        Read {
            #[arg(long)]
            server: String,
            #[arg(long)]
            target: String,
        },
        /// Incremental sync from a feedback server's cursor.
        Sync {
            #[arg(long)]
            server: String,
            #[arg(long)]
            target: String,
            #[arg(long, default_value_t = 0)]
            gt_iat: i64,
        },
    }

    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        let cli = Cli::parse();
        let client = Client::new(ReqwestTransport::new());

        match cli.cmd {
            Cmd::Write {
                server,
                target,
                stars,
                scalar,
                thumb,
                comment,
                key_file,
            } => {
                let body = if let Some(v) = stars {
                    Body::star(v)
                } else if let Some(v) = scalar {
                    Body::scalar(v)
                } else if let Some(up) = thumb {
                    Body::thumb(up)
                } else if let Some(text) = comment {
                    Body::Comment { value: text }
                } else {
                    return Err("provide one of --stars/--scalar/--thumb/--comment".into());
                };
                let motivation = if comment_is_set(&body) {
                    Motivation::Commenting
                } else {
                    Motivation::Assessing
                };
                let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
                let mut ann =
                    Annotation::new(motivation, Target::Iri(target), vec![body]).with_created(now);
                // Self-signed identity: ephemeral by default, or loaded/saved
                // from --key-file so repeated invocations share one issuer.
                let id = match &key_file {
                    Some(path) if path.exists() => {
                        Identity::from_pkcs8_pem(&std::fs::read_to_string(path)?)?
                    }
                    Some(path) => {
                        let id = Identity::generate();
                        std::fs::write(path, id.to_pkcs8_pem()?)?;
                        id
                    }
                    None => Identity::generate(),
                };
                ann.creator = Some(freedback_protocol::Creator::new(id.issuer_id()?));
                id.sign_annotation(&mut ann)?;

                let dest = Dest::Endpoint {
                    point: PublicationPoint::from_server(&server),
                    bearer: None,
                };
                let stored = client.write(&ann, &dest).await?;
                println!("{}", serde_json::to_string_pretty(&stored)?);
            }
            Cmd::Read { server, target } => {
                let anns = client
                    .read(
                        &target,
                        &Source::Endpoint(CollectionPoint::from_server(&server)),
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&anns)?);
            }
            Cmd::Sync {
                server,
                target,
                gt_iat,
            } => {
                let point = CollectionPoint::from_server(&server);
                let anns = client.sync(&point, &target, gt_iat, true).await?;
                println!("{}", serde_json::to_string_pretty(&anns)?);
            }
        }
        Ok(())
    }

    fn comment_is_set(body: &Body) -> bool {
        matches!(body, Body::Comment { .. } | Body::Tag { .. })
    }
}
