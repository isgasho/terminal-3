use std::io::Write;

use crate::{error, Action, Retrieved, Value};

#[cfg(feature = "crosscurses-backend")]
pub(crate) use self::crosscurses::BackendImpl;
#[cfg(feature = "crossterm-backend")]
pub(crate) use self::crossterm::BackendImpl;
#[cfg(feature = "termion-backend")]
pub(crate) use self::termion::BackendImpl;

#[cfg(feature = "crossterm-backend")]
mod crossterm;

#[cfg(feature = "termion-backend")]
mod termion;

#[cfg(feature = "termion-backend")]
mod resize;

#[cfg(feature = "crosscurses-backend")]
mod crosscurses;

/// Interface to an backend library.
pub trait Backend<W: Write> {
    fn create(buffer: W) -> Self;
    fn act(&mut self, action: Action) -> error::Result<()>;
    fn batch(&mut self, action: Action) -> error::Result<()>;
    fn flush_batch(&mut self) -> error::Result<()>;
    fn get(&self, retrieve_operation: Value) -> error::Result<Retrieved>;
}
