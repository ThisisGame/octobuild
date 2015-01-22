extern crate "sha1-hasher" as sha1;

use std::io::{File, IoError, IoErrorKind};

const HEADER: &'static [u8] = b"OBCF\x00\x01";

pub struct Cache {
	cache_dir: Path
}

impl Cache {
	pub fn new() -> Self {
		Cache {
			cache_dir: Path::new(".")
		}
	}

	pub fn run_cached<F: Fn()->Result<(), IoError>>(&self, params: &str, inputs: &Vec<Path>, outputs: &Vec<Path>, worker: F) -> Result<(), IoError> {
		let hash = try! (generate_hash(params, inputs));
		let path = Path::new(".".to_string() + hash.as_slice());
		println!("Cache file: {:?}", path);
		// Try to read data from cache.
		match read_cache(&path, outputs) {
			Ok(_) => {return Ok(())}
			Err(_) => {}
		}
		// Run task and save result to cache.
		try !(worker());
		try !(write_cache(&path, outputs));
		Ok(())
	}
}

// @todo: Need more safe data writing (size before data).
fn generate_hash(params: &str, inputs: &Vec<Path>) -> Result<String, IoError> {
	use std::hash::Writer;

	let mut hash = sha1::Sha1::new();
	// str
	hash.write(params.as_bytes());
	hash.write(&[0]);
	// inputs
	for input in inputs.iter() {
		let content = try! (File::open(input).read_to_end());
		hash.write(content.as_slice());
		hash.write(&[0]);
	}
	Ok(hash.hexdigest())
}

fn write_cache(path: &Path, paths: &Vec<Path>) -> Result<(), IoError> {
	let mut file = try! (File::create(path));
	try! (file.write(HEADER));
	try! (file.write_le_u16(paths.len() as u16));
	for path in paths.iter() {
		let content = try! (File::open(path).read_to_end());
		try! (file.write_le_u32(content.len() as u32));
		try! (file.write(content.as_slice()));			
	}
	Ok(())
}

fn read_cache(path: &Path, paths: &Vec<Path>) -> Result<(), IoError> {
	let mut file = try! (File::open(path));
	if try! (file.read_exact(HEADER.len())) != HEADER {
		return Err(IoError {
			kind: IoErrorKind::InvalidInput,
			desc: "Invalid cache file header",
			detail: Some(path.display().to_string())
		})
	}
	if try! (file.read_le_u16()) as usize != paths.len() {
		return Err(IoError {
			kind: IoErrorKind::InvalidInput,
			desc: "Unexpected count of packed cached files",
			detail: Some(path.display().to_string())
		})
	} 
	for path in paths.iter() {
		let size = try! (file.read_le_u32()) as usize;
		let content = try! (file.read_exact(size));
		try! (File::create(path).write(content.as_slice()));		
	}
	Ok(())
}
