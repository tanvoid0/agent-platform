//! Settings → Account.

use crate::account::{Message, State};
use crate::ui::{self, Icon, Tone};
use iced::Element;

pub fn view(state: &State) -> Element<'_, Message> {
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();

    if let Some(err) = &state.error {
        blocks.push(ui::alert(Tone::Danger, err.clone(), None));
    } else if let Some(note) = &state.notice {
        blocks.push(ui::alert(Tone::Info, note.clone(), None));
    }

    blocks.push(local_card());
    if let Some(session) = &state.session {
        blocks.push(signed_in_card(state, session));
    } else {
        blocks.push(sign_in_card(state));
    }

    ui::page(
        "Account",
        Some(ui::muted(
            "Local work needs no login. Sign in only to use hosted models on the cloud.",
        )),
        None,
        ui::stack_lg(blocks),
    )
}

fn local_card<'a>() -> Element<'a, Message> {
    ui::card_with_header(
        "This machine",
        Some(ui::muted(
            "Projects, chats, coder and local models stay in the SQLite file on this computer. \
             No account. The local API is bound to loopback and needs this install's key.",
        )),
        None,
        ui::stack(vec![ui::caption(
            "Other apps on this machine call http://127.0.0.1:18410 with the key from \
             Settings → Status.",
        )]),
    )
}

fn sign_in_card(state: &State) -> Element<'_, Message> {
    let send = if state.busy {
        ui::badge("waiting…", Tone::Info)
    } else {
        ui::button_default(Icon::Send, "Send magic link", Message::SendLink)
    };
    let mut rows: Vec<Element<'_, Message>> = vec![
        ui::muted(
            "One Portal account covers this app and the store apps. First hosted AI action \
             starts a 14-day trial — no card, no regional price on screen.",
        ),
        ui::field(
            "Cloud URL",
            ui::input("https://api.example.com", &state.url, Message::UrlChanged),
        ),
        ui::field(
            "Email",
            ui::input("you@example.com", &state.email, Message::EmailChanged),
        ),
        ui::cluster(vec![send]).into(),
        ui::separator(),
        ui::caption("Or paste the link from the email (or from the cloud server's logs)."),
        ui::field(
            "Magic link",
            ui::input("https://…/verify?token=…", &state.paste, Message::PasteChanged),
        ),
        ui::cluster(vec![ui::button_outline(Icon::Check, "Verify pasted link", Message::PasteVerify)])
            .into(),
    ];
    if state.waiting {
        rows.insert(
            1,
            ui::alert(
                Tone::Info,
                "Waiting for the email link. Keep this app open.",
                None,
            ),
        );
    }
    ui::card_with_header(
        "Cloud",
        Some(ui::muted("Magic-link sign-in against the hosted API.")),
        None,
        ui::stack(rows),
    )
}

fn signed_in_card<'a>(state: &'a State, session: &'a crate::account::Session) -> Element<'a, Message> {
    let ent = if session.entitlement.is_empty() {
        "unknown".to_string()
    } else {
        session.entitlement.clone()
    };
    let (glyph, tone) = match session.entitlement.as_str() {
        "paid" | "comp" => (Icon::CheckCircle, Tone::Success),
        "trial" => (Icon::Clock, Tone::Info),
        "blocked" => (Icon::XCircle, Tone::Danger),
        _ => (Icon::Info, Tone::Neutral),
    };
    let sign_out = if state.busy {
        ui::badge("signing out…", Tone::Info)
    } else {
        ui::button_outline(Icon::LogOut, "Sign out", Message::SignOut)
    };
    ui::card_with_header(
        "Cloud",
        Some(ui::muted(
            "Hosted models use this session. Local data is still on this machine.",
        )),
        None,
        ui::stack(vec![
            ui::field("Email", ui::body(session.email.clone())),
            ui::field("Entitlement", ui::badge_icon(glyph, ent, tone)),
            ui::field("Origin", ui::mono(session.url.clone())),
            ui::caption(
                "In Chat or Coder, pick provider Platform AI. Everything else — files, \
                 boards, local llama-server — stays here.",
            ),
            ui::cluster(vec![sign_out]).into(),
        ]),
    )
}
