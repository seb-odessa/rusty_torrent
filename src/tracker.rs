extern crate hyper;
extern crate url;

use std::{convert, io};
use std::io::Read;
use self::hyper::Client;
use self::hyper::header::Connection;
use self::url::percent_encoding::{percent_encode, DEFAULT_ENCODE_SET};

use decoder;
use metainfo::Metainfo;
use tracker_response::{Peer, TrackerResponse};

pub fn get_peers(peer_id: &str, meta: &Metainfo, port: u16) -> Result<Vec<Peer>, Error> {
    let length_string = meta.info.length.to_string();
    let encoded_info_hash = percent_encode(&meta.info_hash, DEFAULT_ENCODE_SET).to_string();
    let port_string = port.to_string();
    let params = vec![
        ("left", length_string.as_ref()),
        ("info_hash", encoded_info_hash.as_ref()),
        ("downloaded", "0"),
        ("uploaded", "0"),
        ("event", "started"),
        ("peer_id", peer_id),
        ("compact", "1"),
        ("port", port_string.as_ref()),
    ];

    println!("params: {:?}", &params);

    let url = format!("{}?{}", meta.announce, encode_query_params(&params));
    println!("url: {}", &url);

    let mut client = Client::new();
    let mut http_res = try!(client.get(&url).header(Connection::close()).send());
    println!("http_res: {:?}", &http_res);

    let mut body = Vec::new();
    http_res.read_to_end(&mut body)?;
    println!("body.len(): {:?}", body.len());

    let res = TrackerResponse::parse(&body)?;
    Ok(res.peers)
}

fn encode_query_params(params: &[(&str, &str)]) -> String {
    let param_strings: Vec<String> = params
        .iter()
        .map(|&(k, v)| format!("{}={}", k, v))
        .collect();
    param_strings.join("&")
}

#[derive(Debug)]
pub enum Error {
    DecoderError(decoder::Error),
    HyperError(hyper::Error),
    IoError(io::Error),
}

impl convert::From<decoder::Error> for Error {
    fn from(err: decoder::Error) -> Error {
        Error::DecoderError(err)
    }
}

impl convert::From<hyper::Error> for Error {
    fn from(err: hyper::Error) -> Error {
        Error::HyperError(err)
    }
}

impl convert::From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::IoError(err)
    }
}
