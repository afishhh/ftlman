use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use log::info;
use parking_lot::Mutex;

use crate::{Mod, ModConfigurationState, ModSource, Settings, SharedState, MOD_ORDER_FILENAME};

pub fn scan(settings: Settings, state: Arc<Mutex<SharedState>>, first: bool) -> Result<()> {
    let mut lock = state.lock();

    if lock.locked {
        return Ok(());
    }
    lock.locked = true;

    let old_mod_order_map = std::mem::take(&mut lock.mods)
        .into_iter()
        .enumerate()
        .map(|(i, m)| (m.filename().to_string(), (i, m.enabled)))
        .collect::<HashMap<String, (usize, bool)>>();

    lock.ctx.request_repaint();
    drop(lock);

    let mod_directory = settings.effective_mod_directory();
    let mod_order_map = if first {
        let mod_config_state = match std::fs::File::open(mod_directory.join(MOD_ORDER_FILENAME)) {
            Ok(f) => serde_json::from_reader(std::io::BufReader::new(f))
                .with_context(|| format!("Failed to deserialize mod order from {MOD_ORDER_FILENAME}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut result = ModConfigurationState::default();

                match std::fs::read_to_string(mod_directory.join("modorder.txt")) {
                    Ok(file) => {
                        info!("Importing mod order from Slipstream modorder.txt");
                        for filename in file.lines().map(str::trim).filter(|l| !l.is_empty()) {
                            result.order.0.push(crate::ModOrderElement {
                                filename: filename.to_owned(),
                                enabled: false,
                            });
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e).context("Failed to open slipstream modorder.txt file"),
                }

                result
            }
            Err(e) => return Err(e).context("Failed to open mod order file"),
        };

        state.lock().hyperspace = mod_config_state.hyperspace;

        mod_config_state.order.into_order_map()
    } else {
        old_mod_order_map
    };

    for result in std::fs::read_dir(mod_directory).context("Failed to open mod directory")? {
        let entry = result.context("Failed to read entry from mod directory")?;

        if let Some(mut m) = ModSource::new(&settings, entry.path()).map(Mod::new) {
            let filename = m.filename();
            m.enabled = mod_order_map.get(filename).map_or(false, |x| x.1);

            let mut lock = state.lock();
            lock.mods.push(m);
            lock.mods
                .sort_by_cached_key(|m| mod_order_map.get(m.filename()).map(|x| x.0).unwrap_or(usize::MAX));
            lock.ctx.request_repaint();
        }
    }

    {
        let mut lock = state.lock();
        lock.locked = false;
        lock.ctx.request_repaint();
    }

    Ok(())
}
