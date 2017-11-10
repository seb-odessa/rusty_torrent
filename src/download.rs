use hash::{calculate_sha1, Sha1};
use std::{convert, io};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::sync::mpsc::{Sender, SendError};

use ipc::IPC;
use metainfo::Metainfo;
use request_metadata::RequestMetadata;

pub const BLOCK_SIZE: u32 = 16384;

pub struct Download {
    pub our_peer_id: String,
    pub metainfo: Metainfo,
    pieces: Vec<Piece>,
    file: File,
    peer_channels: Vec<Sender<IPC>>,
}

impl Download {
    pub fn new(our_peer_id: String, metainfo: Metainfo) -> Result<Download, Error> {
        let file_length = metainfo.info.length;
        println!("File length: {}", file_length);
        let piece_length = metainfo.info.piece_length;
        println!("Piece length: {}", piece_length);
        let num_pieces = metainfo.info.num_pieces;
        println!("Number of pieces: {}", metainfo.info.num_pieces);

        // create/open file
        let path = Path::new("./").join(&metainfo.info.name);
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;

        // create pieces
        let mut pieces = vec![];
        for i in 0..num_pieces {
            let offset = i as u64 * piece_length as u64;
            let length = if i < (num_pieces - 1) {
                piece_length
            } else {
                (file_length - offset) as u32
            };
            let mut piece = Piece::new(length, offset, metainfo.info.pieces[i as usize].clone());
            piece.verify(&mut file)?;
            pieces.push(piece);
        }

        Ok(Download {
            our_peer_id: our_peer_id,
            metainfo: metainfo,
            pieces: pieces,
            file: file,
            peer_channels: vec![],
        })
    }

    pub fn register_peer(&mut self, channel: Sender<IPC>) {
        self.peer_channels.push(channel);
    }

    pub fn store(
        &mut self,
        piece_index: u32,
        block_index: u32,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        {
            let piece = &mut self.pieces[piece_index as usize];
            if piece.is_complete || piece.has_block(block_index) {
                // if we already have this block, do an early return to avoid re-writing the piece, sending complete messages, etc
                return Ok(());
            }
            try!(piece.store(&mut self.file, block_index, data));
        }

        // notify peers that this block is complete
        self.broadcast(IPC::BlockComplete(piece_index, block_index));
        println!("Complete {} piece in {} block", piece_index, block_index);

        // notify peers if piece is complete
        if self.pieces[piece_index as usize].is_complete {
            self.broadcast(IPC::PieceComplete(piece_index));
        }
        println!("Complete {} piece", piece_index);

        // notify peers if download is complete
        if self.is_complete() {
            println!("Download complete");
            self.broadcast(IPC::DownloadComplete);
        }

        Ok(())
    }

    pub fn retrive_data(&mut self, request: &RequestMetadata) -> Result<Vec<u8>, Error> {
        let ref piece = self.pieces[request.piece_index as usize];
        if piece.is_complete {
            let offset = piece.offset + request.offset as u64;
            let file = &mut self.file;
            try!(file.seek(io::SeekFrom::Start(offset)));
            let mut buf = vec![];
            try!(file.take(request.block_length as u64).read_to_end(&mut buf));
            Ok(buf)
        } else {
            Err(Error::MissingPieceData)
        }

    }

    pub fn have_pieces(&self) -> Vec<bool> {
        self.pieces.iter().map(|p| p.is_complete).collect()
    }

    pub fn incomplete_blocks_for_piece(&self, piece_index: u32) -> Vec<(u32, u32)> {
        let ref piece = self.pieces[piece_index as usize];
        if !piece.is_complete {
            piece
                .blocks
                .iter()
                .filter(|b| !b.is_complete)
                .map(|b| (b.index, b.length))
                .collect()
        } else {
            vec![]
        }
    }

    fn is_complete(&self) -> bool {
        for piece in self.pieces.iter() {
            if !piece.is_complete {
                return false;
            }
        }
        true
    }

    fn broadcast(&mut self, ipc: IPC) {
        self.peer_channels.retain(|channel| {
            match channel.send(ipc.clone()) {
                Ok(_) => true,
                Err(SendError(_)) => false, // presumably channel has disconnected
            }
        });
    }
}

struct Piece {
    length: u32,
    offset: u64,
    hash: Sha1,
    blocks: Vec<Block>,
    is_complete: bool,
}

impl Piece {
    fn new(length: u32, offset: u64, hash: Sha1) -> Piece {
        // create blocks
        let mut blocks = vec![];
        let num_blocks = (length as f64 / BLOCK_SIZE as f64).ceil() as u32;
        for i in 0..num_blocks {
            let len = if i < (num_blocks - 1) {
                BLOCK_SIZE
            } else {
                length - (BLOCK_SIZE * (num_blocks - 1))
            };
            blocks.push(Block::new(i, len));
        }

        Piece {
            length: length,
            offset: offset,
            hash: hash,
            blocks: blocks,
            is_complete: false,
        }
    }

    fn store(&mut self, file: &mut File, block_index: u32, data: Vec<u8>) -> Result<(), Error> {
        {
            // store data in the appropriate point in the file
            let offset = self.offset + (block_index * BLOCK_SIZE) as u64;
            try!(file.seek(io::SeekFrom::Start(offset)));
            try!(file.write_all(&data));
            self.blocks[block_index as usize].is_complete = true;
        }

        if self.has_all_blocks() {
            let valid = try!(self.verify(file));
            if !valid {
                self.reset_blocks();
            }
        }

        Ok(())
    }

    fn verify(&mut self, file: &mut File) -> Result<bool, Error> {
        // read in the part of the file corresponding to the piece
        file.seek(io::SeekFrom::Start(self.offset))?;
        let mut data = vec![];
        file.take(self.length as u64).read_to_end(&mut data)?;
        // calculate the hash, verify it, and update is_complete
        self.is_complete = self.hash == calculate_sha1(&data);
        Ok(self.is_complete)
    }

    fn has_block(&self, block_index: u32) -> bool {
        self.blocks[block_index as usize].is_complete
    }

    fn has_all_blocks(&self) -> bool {
        for block in self.blocks.iter() {
            if !block.is_complete {
                return false;
            }
        }
        true
    }

    fn reset_blocks(&mut self) {
        for block in self.blocks.iter_mut() {
            block.is_complete = false;
        }
    }
}

struct Block {
    index: u32,
    length: u32,
    is_complete: bool,
}

impl Block {
    fn new(index: u32, length: u32) -> Block {
        Block {
            index: index,
            length: length,
            is_complete: false,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    MissingPieceData,
    IoError(io::Error),
}

impl convert::From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::IoError(err)
    }
}
