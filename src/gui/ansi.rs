use eframe::egui::{
    text::{LayoutJob, LayoutSection},
    Color32, FontId, TextFormat,
};
use log::warn;

pub fn layout_ansi(output: &mut LayoutJob, text: &str, font_id: FontId) {
    let mut last = 0;
    let default_style = TextFormat {
        font_id,
        ..Default::default()
    };
    let mut current_style = default_style.clone();

    while let Some(sequence) = text.bytes().skip(last).position(|b| b == b'\x1b').map(|off| last + off) {
        if last != sequence {
            let start = output.text.len();
            output.text.push_str(&text[last..sequence]);
            output.sections.push(LayoutSection {
                leading_space: 0.0,
                byte_range: start..output.text.len(),
                format: current_style.clone(),
            });
        }

        match text.as_bytes().get(sequence + 1) {
            Some(b'[') => (),
            Some(_) => {
                last = sequence + 2;
                continue;
            }
            None => {
                last = sequence + 1;
                continue;
            }
        }

        let Some(len) = text.bytes().skip(sequence).position(|b| b == b'm') else {
            last = sequence + 2;
            continue;
        };

        let commands = &text[sequence + 2..sequence + len];

        let it = commands.split(':');
        for command in it {
            match command.parse::<u8>() {
                Ok(0) => current_style.clone_from(&default_style),
                Ok(1) => (), // bold not supported by egui
                Ok(30) => current_style.color = Color32::BLACK,
                Ok(31) => current_style.color = Color32::RED,
                Ok(91) => current_style.color = Color32::LIGHT_RED,
                Ok(32) => current_style.color = Color32::GREEN,
                Ok(33) => current_style.color = Color32::YELLOW,
                Ok(34) => current_style.color = Color32::BLUE,
                Ok(94) => current_style.color = Color32::LIGHT_BLUE,
                Ok(35) => current_style.color = Color32::from_rgb(255, 0, 255),
                Ok(36) => current_style.color = Color32::from_rgb(0, 255, 255),
                Ok(37) => current_style.color = Color32::WHITE,
                Ok(39) => current_style.color = Color32::GRAY,
                Ok(i) => warn!("unrecognised ANSI escape command {i}"),
                Err(_) => (),
            }
        }

        last = sequence + len + 1;
    }

    if last != text.len() {
        let start = output.text.len();
        output.text.push_str(&text[last..]);
        output.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: start..output.text.len(),
            format: current_style,
        });
    }
}
