use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use parking_lot::Mutex;

use crate::{Mod, ModConfigurationState, ModSource, Settings, SharedState, MOD_ORDER_FILENAME};

pub fn scan(settings: Settings, state: Arc<Mutex<SharedState>>, first: bool) -> Result<()> {
    let mut lock = state.lock();

    if lock.locked {
        return Ok(());
    }
    lock.locked = true;

    let old = std::mem::take(&mut lock.mods)
        .into_iter()
        .map(|m| (m.filename().to_string(), m))
        .collect::<HashMap<String, Mod>>();

    lock.ctx.request_repaint();
    drop(lock);

    let mod_config_state = match std::fs::File::open(settings.mod_directory.join(MOD_ORDER_FILENAME)) {
        Ok(f) => serde_json::from_reader(std::io::BufReader::new(f))
            .with_context(|| format!("Failed to deserialize mod order from {MOD_ORDER_FILENAME}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ModConfigurationState::default(),
        Err(e) => return Err(e).context("Failed to open mod order file"),
    };
    if first {
        state.lock().hyperspace = mod_config_state.hyperspace;
    }
    let mod_order_map = mod_config_state.order.into_order_map();

    for result in std::fs::read_dir(&settings.mod_directory).context("Failed to open mod directory")? {
        let entry = result.context("Failed to read entry from mod directory")?;

        if let Some(mut m) = ModSource::new(&settings, entry.path()).map(Mod::new) {
            let filename = m.filename();
            m.enabled = old
                .get(filename)
                .map_or(mod_order_map.get(filename).map(|x| x.1).unwrap_or(false), |o| o.enabled);

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
