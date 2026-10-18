use std::{env, time::Duration};

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use thiserror::Error;

use super::login::Email;

pub(crate) struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

#[derive(Debug, Error)]
pub(crate) enum MailError {
    #[error("invalid SMTP configuration: {0}")]
    Configuration(&'static str),
    #[error("could not construct invitation email")]
    Message,
    #[error("invitation delivery failed")]
    Delivery,
}

impl Mailer {
    pub(crate) fn from_env() -> Result<Self, MailError> {
        let host = setting("TURNESTA_SMTP_HOST", "mailhog")?;
        let port: u16 = setting("TURNESTA_SMTP_PORT", "1025")?
            .parse()
            .ok()
            .filter(|port| *port > 0)
            .ok_or(MailError::Configuration("TURNESTA_SMTP_PORT"))?;
        let mode = setting("TURNESTA_SMTP_TLS_MODE", "smtps")?;
        let allow_insecure = match setting("TURNESTA_SMTP_ALLOW_INSECURE", "false")?.as_str() {
            "true" => true,
            "false" => false,
            _ => return Err(MailError::Configuration("TURNESTA_SMTP_ALLOW_INSECURE")),
        };
        let mut builder = match mode.as_str() {
            "smtps" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)
                .map_err(|_| MailError::Configuration("SMTP TLS host"))?,
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
                .map_err(|_| MailError::Configuration("SMTP TLS host"))?,
            "insecure" if allow_insecure && local_host(&host) => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host)
            }
            _ => {
                return Err(MailError::Configuration(
                    "SMTP TLS mode or local insecure permission",
                ));
            }
        }
        .port(port)
        .timeout(Some(Duration::from_secs(10)));

        match (
            optional("TURNESTA_SMTP_USERNAME")?,
            optional("TURNESTA_SMTP_PASSWORD")?,
        ) {
            (Some(username), Some(password)) => {
                if mode == "insecure" {
                    return Err(MailError::Configuration("SMTP credentials require TLS"));
                }
                builder = builder.credentials(Credentials::new(username, password));
            }
            (None, None) => {}
            _ => {
                return Err(MailError::Configuration(
                    "SMTP username and password must be supplied together",
                ));
            }
        }
        let from = setting("TURNESTA_SMTP_FROM", "no-reply@turnesta.test")?
            .parse()
            .map_err(|_| MailError::Configuration("TURNESTA_SMTP_FROM"))?;
        Ok(Self {
            transport: builder.build(),
            from,
        })
    }

    pub(crate) async fn send_invitation(
        &self,
        recipient: &Email,
        login_link: &str,
    ) -> Result<(), MailError> {
        let message = Message::builder()
            .from(self.from.clone())
            .to(recipient.as_str().parse().map_err(|_| MailError::Message)?)
            .subject("Sign in to Turnesta")
            .header(ContentType::TEXT_PLAIN)
            .body(format!(
                "You have been invited to Turnesta.\n\nOpen this link and confirm sign-in:\n{login_link}\n\n\
                 This link expires 10 minutes after it was issued and can be used once.\n\
                 If you did not expect this invitation, ignore this email.\n"
            ))
            .map_err(|_| MailError::Message)?;
        // SMTP servers can include submitted data in errors. Keep the transport
        // error private instead of forwarding it into logs or CLI diagnostics.
        tokio::time::timeout(Duration::from_secs(15), self.transport.send(message))
            .await
            .map_err(|_| MailError::Delivery)?
            .map_err(|_| MailError::Delivery)?;
        Ok(())
    }
}

fn setting(name: &'static str, default: &str) -> Result<String, MailError> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) => Err(MailError::Configuration(name)),
        Err(env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(env::VarError::NotUnicode(_)) => Err(MailError::Configuration(name)),
    }
}

fn optional(name: &'static str) -> Result<Option<String>, MailError> {
    match env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(MailError::Configuration(name)),
    }
}

fn local_host(host: &str) -> bool {
    matches!(
        host,
        "mailhog" | "localhost" | "127.0.0.1" | "::1" | "host.docker.internal"
    )
}
