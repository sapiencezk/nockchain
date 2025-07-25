use nockvm::noun::{IndirectAtom, Noun, D, NO, T, YES};
use nockvm_macros::tas;
use tracing::{debug, error};

use crate::nockapp::driver::{make_driver, IODriverFn};
use crate::nockapp::wire::{Wire, WireRepr};
use crate::noun::slab::NounSlab;
use crate::noun::FromAtom;
use crate::AtomExt;

pub enum FileWire {
    Read,
    Write,
}

impl Wire for FileWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "file";

    fn to_wire(&self) -> crate::nockapp::wire::WireRepr {
        let tags = match self {
            FileWire::Read => vec!["read".into()],
            FileWire::Write => vec!["write".into()],
        };
        WireRepr::new(FileWire::SOURCE, FileWire::VERSION, tags)
    }
}

/// File IO Driver
///
/// ## Effects
/// `[%file %read path=@t]`
/// results in poke
/// `[%file %read ~]` on read error
/// or
/// `[%file %read ~ contents=@]` on read success
///
///  `[%file %write path=@t contents=@]`
///  results in file written to disk and poke
///  `[%file %write path=@t contents=@ success=?]`
pub fn file() -> IODriverFn {
    make_driver(|handle| async move {
        loop {
            let effect_res = handle.next_effect().await;
            let slab = match effect_res {
                Ok(slab) => slab,
                Err(e) => {
                    error!("Error receiving effect: {:?}", e);
                    continue;
                }
            };

            let Ok(effect_cell) = unsafe { slab.root() }.as_cell() else {
                continue;
            };

            if !unsafe { effect_cell.head().raw_equals(&D(tas!(b"file"))) } {
                continue;
            }

            let Ok(file_cell) = effect_cell.tail().as_cell() else {
                continue;
            };

            let (operation, path_atom) = match file_cell.head().as_direct() {
                Ok(tag) if tag.data() == tas!(b"read") => ("read", file_cell.tail().as_atom().ok()),
                Ok(tag) if tag.data() == tas!(b"write") => {
                    let Ok(write_cell) = file_cell.tail().as_cell() else {
                        continue;
                    };
                    ("write", write_cell.head().as_atom().ok())
                }
                _ => continue,
            };

            match (operation, path_atom) {
                ("read", Some(path_atom)) => {
                    let path = String::from_utf8(Vec::from(path_atom.as_ne_bytes()))?;
                    match tokio::fs::read(&path).await {
                        Ok(contents) => {
                            let mut poke_slab = NounSlab::new();
                            let contents_atom = unsafe {
                                IndirectAtom::new_raw_bytes_ref(&mut poke_slab, &contents)
                                    .normalize_as_atom()
                            };
                            let contents_noun = Noun::from_atom(contents_atom);
                            let poke_noun = T(
                                &mut poke_slab,
                                &[D(tas!(b"file")), D(tas!(b"read")), D(0), contents_noun],
                            );
                            poke_slab.set_root(poke_noun);
                            let wire = FileWire::Read.to_wire();
                            handle.poke(wire, poke_slab).await?;
                        }
                        Err(_) => {
                            let mut poke_slab = NounSlab::new();
                            let poke_noun =
                                T(&mut poke_slab, &[D(tas!(b"file")), D(tas!(b"read")), D(0)]);
                            poke_slab.set_root(poke_noun);
                            let wire = FileWire::Read.to_wire();
                            handle.poke(wire, poke_slab).await?;
                        }
                    }
                }
                ("write", Some(path_atom)) => {
                    let Ok(write_cell) = file_cell.tail().as_cell() else {
                        continue;
                    };
                    let Ok(contents_atom) = write_cell.tail().as_atom() else {
                        continue;
                    };
                    let path = path_atom.into_string()?;
                    let contents = contents_atom.as_ne_bytes();
                    debug!("file driver: writing {} bytes to: {}", contents.len(), path);

                    // Create parent directories if they don't exist
                    if let Some(parent) = std::path::Path::new(&path).parent() {
                        if let Err(e) = tokio::fs::create_dir_all(parent).await {
                            error!("file driver: error creating directories: {}", e);
                            let mut poke_slab = NounSlab::new();
                            let poke_noun = T(
                                &mut poke_slab,
                                &[
                                    D(tas!(b"file")),
                                    D(tas!(b"write")),
                                    path_atom.as_noun(),
                                    contents_atom.as_noun(),
                                    NO,
                                ],
                            );
                            poke_slab.set_root(poke_noun);
                            let wire = FileWire::Write.to_wire();
                            handle.poke(wire, poke_slab).await?;
                            continue;
                        }
                    }

                    match tokio::fs::write(&path, contents).await {
                        Ok(_) => {
                            let mut poke_slab = NounSlab::new();
                            let poke_noun = T(
                                &mut poke_slab,
                                &[
                                    D(tas!(b"file")),
                                    D(tas!(b"write")),
                                    path_atom.as_noun(),
                                    contents_atom.as_noun(),
                                    YES,
                                ],
                            );
                            poke_slab.set_root(poke_noun);
                            let wire = FileWire::Write.to_wire();
                            handle.poke(wire, poke_slab).await?;
                        }
                        Err(e) => {
                            error!("file driver: error writing to path: {}", e);
                            let mut poke_slab = NounSlab::new();
                            let poke_noun = T(
                                &mut poke_slab,
                                &[
                                    D(tas!(b"file")),
                                    D(tas!(b"write")),
                                    path_atom.as_noun(),
                                    contents_atom.as_noun(),
                                    NO,
                                ],
                            );
                            poke_slab.set_root(poke_noun);
                            let wire = FileWire::Write.to_wire();
                            handle.poke(wire, poke_slab).await?;
                        }
                    }
                }
                _ => continue,
            }
        }
    })
}
