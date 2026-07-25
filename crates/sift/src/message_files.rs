//! Native file-dialog and filesystem integration for message payloads and
//! reusable templates.

use sift_core::body::BodyFormat;
use sift_core::message::SiftMessage;
use sift_core::message_file::MessageFile;

use crate::ui::send_dialog::SendDialog;

pub fn load_payload(dialog: &mut SendDialog) {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Open message payload")
        .add_filter(
            "Message payloads",
            &["json", "xml", "txt", "bin", "dat", "gz"],
        )
        .pick_file()
    else {
        return;
    };
    match std::fs::read(&path) {
        Ok(bytes) => dialog.load_payload(bytes),
        Err(error) => {
            dialog.error = Some(format!("Could not read '{}': {error}", path.display()));
        }
    }
}

pub fn load_template(dialog: &mut SendDialog) {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Open sift message template")
        .add_filter("Sift message template", &["json"])
        .pick_file()
    else {
        return;
    };
    let result = std::fs::read_to_string(&path)
        .map_err(|error| format!("Could not read '{}': {error}", path.display()))
        .and_then(|json| {
            MessageFile::from_json(&json)
                .map_err(|error| error.to_string())
                .and_then(|file| file.to_outbound().map_err(|error| error.to_string()))
        });
    match result {
        Ok(message) => dialog.load_message(message),
        Err(error) => dialog.error = Some(error),
    }
}

/// Save the current composed message. `Ok(false)` means validation failed or
/// the user cancelled the picker, so the dialog should remain open.
pub fn save_composed_template(dialog: &mut SendDialog) -> Result<bool, String> {
    let Some(message) = dialog.build_message() else {
        return Ok(false);
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("Save sift message template")
        .add_filter("Sift message template", &["json"])
        .set_file_name("message.sift-message.json")
        .save_file()
    else {
        return Ok(false);
    };
    let json = MessageFile::from_outbound(&message)
        .to_json()
        .map_err(|error| error.to_string())?;
    std::fs::write(&path, json)
        .map_err(|error| format!("Could not write '{}': {error}", path.display()))?;
    dialog.error = None;
    Ok(true)
}

/// Save the selected message body. `Ok(false)` means the picker was cancelled.
pub fn save_body(message: &SiftMessage) -> Result<bool, String> {
    let extension = body_extension(message);
    let file_name = format!("{}.{}", file_stem(message), extension);
    let Some(path) = rfd::FileDialog::new()
        .set_title("Save message body")
        .set_file_name(file_name)
        .save_file()
    else {
        return Ok(false);
    };
    let bytes = if message.body.bytes.is_empty() {
        message.body.text.as_deref().unwrap_or_default().as_bytes()
    } else {
        &message.body.bytes
    };
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Could not write '{}': {error}", path.display()))?;
    Ok(true)
}

/// Save a reusable template from the selected message. `Ok(false)` means the
/// picker was cancelled.
pub fn save_template(message: &SiftMessage) -> Result<bool, String> {
    let file_name = format!("{}.sift-message.json", file_stem(message));
    let Some(path) = rfd::FileDialog::new()
        .set_title("Save sift message template")
        .add_filter("Sift message template", &["json"])
        .set_file_name(file_name)
        .save_file()
    else {
        return Ok(false);
    };
    let json = MessageFile::from_message(message)
        .to_json()
        .map_err(|error| error.to_string())?;
    std::fs::write(&path, json)
        .map_err(|error| format!("Could not write '{}': {error}", path.display()))?;
    Ok(true)
}

fn file_stem(message: &SiftMessage) -> String {
    let raw = message
        .message_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map_or_else(
            || format!("message-{}", message.sequence_number),
            str::to_owned,
        );
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        format!("message-{}", message.sequence_number)
    } else {
        sanitized
    }
}

fn body_extension(message: &SiftMessage) -> &'static str {
    if message.body.gzipped {
        return "gz";
    }
    match message.body.format {
        BodyFormat::Json => "json",
        BodyFormat::Xml => "xml",
        BodyFormat::Text | BodyFormat::AmqpValue => "txt",
        BodyFormat::Binary | BodyFormat::Empty => "bin",
    }
}
