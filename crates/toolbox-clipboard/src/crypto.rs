//! Noise NNpsk0 transport and the pre-shared key file.

use rand::RngCore;
use snow::{Builder, TransportState, params::NoiseParams};
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::Path,
};

pub const KEY_BYTES: usize = 32;
const MAX_HANDSHAKE: usize = 65535;

#[derive(Debug)]
pub enum CryptoError {
    Io(std::io::Error),
    Noise(snow::Error),
    InvalidKey(&'static str),
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O: {error}"),
            Self::Noise(error) => write!(f, "Noise: {error}"),
            Self::InvalidKey(reason) => write!(f, "invalid key: {reason}"),
        }
    }
}

impl std::error::Error for CryptoError {}

impl From<std::io::Error> for CryptoError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<snow::Error> for CryptoError {
    fn from(error: snow::Error) -> Self {
        Self::Noise(error)
    }
}

fn params() -> NoiseParams {
    "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s"
        .parse()
        .expect("built-in Noise pattern")
}

pub fn generate_key() -> [u8; KEY_BYTES] {
    let mut key = [0u8; KEY_BYTES];
    rand::rng().fill_bytes(&mut key);
    key
}

pub fn read_key(path: &Path) -> Result<[u8; KEY_BYTES], CryptoError> {
    let bytes = fs::read(path)?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidKey("expected 32 bytes"))
}

pub fn write_key(path: &Path, key: &[u8; KEY_BYTES]) -> Result<(), CryptoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(key)?;
    file.sync_all()?;
    Ok(())
}

pub struct Session {
    transport: TransportState,
}

impl Session {
    pub fn initiator(stream: &mut TcpStream, psk: &[u8; KEY_BYTES]) -> Result<Self, CryptoError> {
        let mut handshake = Builder::new(params()).psk(0, psk).build_initiator()?;
        let mut buffer = vec![0u8; MAX_HANDSHAKE];
        let len = handshake.write_message(&[], &mut buffer)?;
        write_handshake(stream, &buffer[..len])?;
        let message = read_handshake(stream)?;
        handshake.read_message(&message, &mut buffer)?;
        Ok(Self {
            transport: handshake.into_transport_mode()?,
        })
    }

    pub fn responder(stream: &mut TcpStream, psk: &[u8; KEY_BYTES]) -> Result<Self, CryptoError> {
        let mut handshake = Builder::new(params()).psk(0, psk).build_responder()?;
        let message = read_handshake(stream)?;
        let mut buffer = vec![0u8; MAX_HANDSHAKE];
        handshake.read_message(&message, &mut buffer)?;
        let len = handshake.write_message(&[], &mut buffer)?;
        write_handshake(stream, &buffer[..len])?;
        Ok(Self {
            transport: handshake.into_transport_mode()?,
        })
    }

    pub fn send(&mut self, stream: &mut TcpStream, plaintext: &[u8]) -> Result<(), CryptoError> {
        let ciphertext = self.seal(plaintext)?;
        let len = u16::try_from(ciphertext.len())
            .map_err(|_| CryptoError::InvalidKey("ciphertext too large"))?;
        stream.write_all(&len.to_be_bytes())?;
        stream.write_all(&ciphertext)?;
        stream.flush()?;
        Ok(())
    }

    pub fn receive(&mut self, stream: &mut TcpStream) -> Result<Vec<u8>, CryptoError> {
        let mut length = [0u8; 2];
        stream.read_exact(&mut length)?;
        let length = u16::from_be_bytes(length) as usize;
        if !(16..=MAX_HANDSHAKE).contains(&length) {
            return Err(CryptoError::InvalidKey("invalid ciphertext length"));
        }
        let mut ciphertext = vec![0u8; length];
        stream.read_exact(&mut ciphertext)?;
        self.open(&ciphertext)
    }

    /// Encrypt one transport message. The caller owns the outer length prefix.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if plaintext.len() + 16 > MAX_HANDSHAKE {
            return Err(CryptoError::InvalidKey("transport frame too large"));
        }
        let mut ciphertext = vec![0u8; plaintext.len() + 16];
        let len = self.transport.write_message(plaintext, &mut ciphertext)?;
        ciphertext.truncate(len);
        Ok(ciphertext)
    }

    /// Decrypt one transport message.
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if ciphertext.len() < 16 || ciphertext.len() > MAX_HANDSHAKE {
            return Err(CryptoError::InvalidKey("invalid ciphertext length"));
        }
        let mut plaintext = vec![0u8; MAX_HANDSHAKE];
        let len = self.transport.read_message(ciphertext, &mut plaintext)?;
        plaintext.truncate(len);
        Ok(plaintext)
    }
}

fn write_handshake(stream: &mut TcpStream, message: &[u8]) -> Result<(), CryptoError> {
    let length = u16::try_from(message.len())
        .map_err(|_| CryptoError::InvalidKey("handshake message too large"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(message)?;
    stream.flush()?;
    Ok(())
}

fn read_handshake(stream: &mut TcpStream) -> Result<Vec<u8>, CryptoError> {
    let mut length = [0u8; 2];
    stream.read_exact(&mut length)?;
    let length = u16::from_be_bytes(length) as usize;
    let mut message = vec![0u8; length];
    stream.read_exact(&mut message)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn stream_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = thread::spawn(move || TcpStream::connect(address).unwrap());
        let (server, _) = listener.accept().unwrap();
        let client = client.join().unwrap();
        (client, server)
    }

    #[test]
    fn key_file_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard.key");
        let key = generate_key();
        write_key(&path, &key).unwrap();
        assert_eq!(read_key(&path).unwrap(), key);
        assert!(write_key(&path, &key).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn noise_handshake_and_message_round_trip() {
        let key = generate_key();
        let (mut client, mut server) = stream_pair();
        let server_thread = thread::spawn(move || {
            let mut session = Session::responder(&mut server, &key).unwrap();
            let message = session.receive(&mut server).unwrap();
            assert_eq!(message, b"hello");
            session.send(&mut server, b"world").unwrap();
        });
        let mut session = Session::initiator(&mut client, &key).unwrap();
        session.send(&mut client, b"hello").unwrap();
        assert_eq!(session.receive(&mut client).unwrap(), b"world");
        server_thread.join().unwrap();
    }

    #[test]
    fn wrong_key_is_rejected() {
        let key = generate_key();
        let other = generate_key();
        let (mut client, mut server) = stream_pair();
        let server_thread = thread::spawn(move || {
            let _ = Session::responder(&mut server, &other);
        });
        assert!(Session::initiator(&mut client, &key).is_err());
        server_thread.join().unwrap();
    }
}
