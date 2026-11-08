use crate::app::Icon;
use crate::layout::Position;
use std::{collections::HashMap, fs, io, path::Path};

pub fn load_positions(path: &Path) -> HashMap<String, Position> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_positions(path: &Path, icons: &[Icon]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let positions: HashMap<_, _> = icons
        .iter()
        .map(|i| (i.path.to_string_lossy().into_owned(), i.position))
        .collect();
    let data = serde_json::to_vec_pretty(&positions).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip_as_json() {
        let mut positions = HashMap::new();
        positions.insert("/tmp/example".to_owned(), Position { x: 50, y: 180 });
        let encoded = serde_json::to_string(&positions).expect("serialize positions");
        let decoded: HashMap<String, Position> =
            serde_json::from_str(&encoded).expect("deserialize positions");
        assert_eq!(decoded["/tmp/example"].x, 50);
        assert_eq!(decoded["/tmp/example"].y, 180);
    }
}
