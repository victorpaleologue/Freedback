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
        /// Sign and publish a rating/comment/issue to a feedback server.
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
            /// Report an issue / problem with the target (free text). Emits an
            /// `oa:TextualBody` under the standard `oa:editing` motivation —
            /// the problem-report feedback type (ADR 0023). Mutually exclusive
            /// with the other body flags.
            #[arg(long)]
            issue: Option<String>,
            /// License IRI to distribute this feedback under (sets the W3C
            /// annotation `rights` property, e.g.
            /// `https://creativecommons.org/licenses/by/4.0/`). Optional —
            /// without it the feedback falls under the server's default
            /// license, advertised in `/.well-known/freedback` (ADR 0022).
            #[arg(long)]
            license: Option<String>,
            /// Reuse (or create) a persistent identity instead of a fresh
            /// throwaway one — load the PKCS#8 PEM keypair from this file if
            /// it exists, otherwise generate one and save it here. Lets later
            /// calls act as the SAME issuer: a newer `write` for the same
            /// target supersedes the older one (edit), and `delete` erases a
            /// post outright — it must be signed with the same key that wrote
            /// it (right to erasure, ADR 0021).
            #[arg(long)]
            key_file: Option<std::path::PathBuf>,
        },
        /// Erase a previously published annotation (right to erasure, ADR
        /// 0021). Signs a delete document with the SAME key that signed the
        /// annotation; the server removes the content and keeps only a
        /// content-free tombstone.
        Delete {
            #[arg(long)]
            server: String,
            /// The annotation's dedup id, or the full `…/annotations/<id>`
            /// URL as printed by `write`.
            #[arg(long)]
            id: String,
            /// The PKCS#8 PEM keypair that signed the annotation (the file
            /// `write --key-file` saved). A different key is refused (403).
            #[arg(long)]
            key_file: std::path::PathBuf,
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
                issue,
                license,
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
                } else if let Some(text) = issue {
                    Body::issue(text)
                } else {
                    return Err("provide one of --stars/--scalar/--thumb/--comment/--issue".into());
                };
                let motivation = motivation_for(&body);
                let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
                let mut ann =
                    Annotation::new(motivation, Target::Iri(target), vec![body]).with_created(now);
                // Explicit data license (ADR 0022): part of the content, so it
                // must be set BEFORE signing (it participates in the canonical
                // bytes the signature covers).
                if let Some(license) = license {
                    ann = ann.with_rights(license);
                }
                // Self-signed identity: ephemeral by default, or loaded/saved
                // from --key-file so repeated invocations share one issuer.
                let id = match &key_file {
                    Some(path) => load_or_init_identity(path)?,
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
            Cmd::Delete {
                server,
                id,
                key_file,
            } => {
                let dedup = freedback_cli_client::dedup_id_from_url(&id).to_string();
                let identity = load_or_init_identity(&key_file)?;
                let now = OffsetDateTime::now_utc().format(&Rfc3339)?;
                let mut doc = freedback_protocol::DeleteRequest::new(&dedup, now);
                identity.sign_delete(&mut doc)?;
                client
                    .delete(&PublicationPoint::from_server(&server), &doc, None)
                    .await?;
                println!("deleted {dedup}");
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

    /// The motivation matching each body kind: textual bodies carry the
    /// motivation their purpose names; every rating motivates `assessing`.
    fn motivation_for(body: &Body) -> Motivation {
        match body {
            Body::Comment { .. } => Motivation::Commenting,
            Body::Tag { .. } => Motivation::Tagging,
            Body::Issue { .. } => Motivation::Editing,
            _ => Motivation::Assessing,
        }
    }

    /// The `--key-file` mechanism shared by `write` and `delete`: load the
    /// PKCS#8 PEM keypair from `path` if it exists, otherwise generate a fresh
    /// identity and save it there.
    fn load_or_init_identity(
        path: &std::path::Path,
    ) -> Result<Identity, Box<dyn std::error::Error>> {
        if path.exists() {
            Ok(Identity::from_pkcs8_pem(&std::fs::read_to_string(path)?)?)
        } else {
            let id = Identity::generate();
            std::fs::write(path, id.to_pkcs8_pem()?)?;
            Ok(id)
        }
    }
}
