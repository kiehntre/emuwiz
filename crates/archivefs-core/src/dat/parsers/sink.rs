//! Shared destination for the collected and incremental parser paths.
use crate::dat::model::DatGameEntry;
use crate::dat::parser::ParseError;

pub(super) type Visitor<'a> = dyn FnMut(DatGameEntry) -> Result<(), ParseError> + 'a;

pub(super) struct EntrySink<'a> {
    visitor: Option<&'a mut Visitor<'a>>,
    games: Vec<DatGameEntry>,
    count: usize,
    rom_count: usize,
}

impl<'a> EntrySink<'a> {
    pub(super) fn collect() -> Self {
        Self {
            visitor: None,
            games: Vec::new(),
            count: 0,
            rom_count: 0,
        }
    }

    pub(super) fn visit(visitor: &'a mut Visitor<'a>) -> Self {
        Self {
            visitor: Some(visitor),
            ..Self::collect()
        }
    }

    pub(super) fn len(&self) -> usize {
        self.count
    }
    pub(super) fn rom_count(&self) -> usize {
        self.rom_count
    }

    pub(super) fn push(&mut self, game: DatGameEntry) -> Result<(), ParseError> {
        self.rom_count += game.roms.len()
            + game
                .parts
                .iter()
                .flat_map(|part| &part.data_areas)
                .map(|area| area.roms.len())
                .sum::<usize>();
        self.count += 1;
        if let Some(visitor) = &mut self.visitor {
            visitor(game)?;
        } else {
            self.games.push(game);
        }
        Ok(())
    }

    pub(super) fn into_games(self) -> Vec<DatGameEntry> {
        self.games
    }
}
