use std::{ffi::c_char, mem::zeroed, path::PathBuf, ptr::null_mut, slice::from_raw_parts};

use libflac_sys::*;

pub struct FlacBuilder<'data, Sample>
where
    Sample: IntoSample,
{
    data: &'data Vec<Vec<Sample>>,
    bps: BpsLevel,
    sample_rate: u32,
    output_path: Option<String>,
    compression_level: u32,
    padding: u32,
    vorbis_commenets: Vec<(String, String)>,
    metadata_blocks: Vec<*mut FLAC__StreamMetadata>,
}

impl<'data, Sample: IntoSample> FlacBuilder<'data, Sample> {
    pub fn new(data: &'data Vec<Vec<Sample>>, sample_rate: u32) -> Self {
        FlacBuilder {
            data,
            sample_rate,
            bps: BpsLevel::Bps16,
            compression_level: 5,
            padding: 500,
            vorbis_commenets: vec![],
            metadata_blocks: vec![],
            output_path: None,
        }
    }

    pub fn compression_level(mut self, level: u32) -> Self {
        self.compression_level = level;
        self
    }

    pub fn bps(mut self, bps: BpsLevel) -> Self {
        self.bps = bps;
        self
    }

    pub fn padding(mut self, padding: u32) -> Self {
        self.padding = padding;
        self
    }

    pub fn artist(self, artist: &str) -> Self {
        self.vorbis_comment("ARTIST", artist)
    }

    pub fn year(self, year: u32) -> Self {
        self.vorbis_comment("YEAR", &year.to_string())
    }

    pub fn vorbis_comment(mut self, key: &str, value: &str) -> Self {
        self.vorbis_commenets
            .push((format!("{}\0", key), format!("{}\0", value)));
        self
    }

    pub fn output_path(mut self, path: PathBuf) -> Self {
        self.output_path = Some(format!("{}\0", path.display()).to_string());
        self
    }

    fn total_samples(&self) -> usize {
        self.data.iter().map(|channel| channel.len()).sum()
    }

    unsafe fn prepare(&mut self) -> Result<*mut FLAC__StreamEncoder, EncoderError> {
        if self.data.is_empty() {
            return Err(EncoderError::NoData);
        }

        let encoder = FLAC__stream_encoder_new();

        if encoder.is_null() {
            return Err(EncoderError::InitializationError);
        }

        if 0 == FLAC__stream_encoder_set_verify(encoder, 1) {
            return Err(EncoderError::VerificationError);
        }

        if 0 == FLAC__stream_encoder_set_compression_level(encoder, self.compression_level) {
            return Err(EncoderError::InvalidCompressionLevel);
        }

        let channels = self.data.len();

        if 0 == FLAC__stream_encoder_set_channels(encoder, channels as u32) {
            return Err(EncoderError::InvalidChannelCount);
        }

        if 0 == FLAC__stream_encoder_set_bits_per_sample(encoder, self.bps.to_u32()) {
            return Err(EncoderError::InvalidSampleType);
        }

        if 0 == FLAC__stream_encoder_set_sample_rate(encoder, self.sample_rate) {
            return Err(EncoderError::InvalidSampleRate);
        }

        if 0 == FLAC__stream_encoder_set_total_samples_estimate(
            encoder,
            self.total_samples() as u64,
        ) {
            return Err(EncoderError::TooManyOrTooFewSamples);
        }

        // Metadata

        if self.vorbis_commenets.is_empty() {
            if 0 == FLAC__stream_encoder_set_metadata(encoder, null_mut(), 0) {
                return Err(EncoderError::FailedToSetMetadata);
            }
        }

        if !self.vorbis_commenets.is_empty() {
            let metadata_block = FLAC__metadata_object_new(FLAC__METADATA_TYPE_VORBIS_COMMENT);

            if metadata_block.is_null() {
                return Err(EncoderError::InitializationError);
            }

            for (key, value) in &self.vorbis_commenets {
                let mut entry: FLAC__StreamMetadata_VorbisComment_Entry = zeroed();

                if 0 == FLAC__metadata_object_vorbiscomment_entry_from_name_value_pair(
                    &mut entry,
                    key.as_bytes().as_ptr() as *const c_char,
                    value.as_bytes().as_ptr() as *const c_char,
                ) {
                    return Err(EncoderError::InvalidVorbisComment(key.clone()));
                }

                if 0 == FLAC__metadata_object_vorbiscomment_append_comment(metadata_block, entry, 0)
                {
                    return Err(EncoderError::FailedToSetMetadata);
                }
            }

            self.metadata_blocks.push(metadata_block);
        }

        let padding_block = FLAC__metadata_object_new(FLAC__METADATA_TYPE_PADDING);
        if !padding_block.is_null() {
            (*padding_block).length = self.padding;
            self.metadata_blocks.push(padding_block);
        }

        if 0 == FLAC__stream_encoder_set_metadata(
            encoder,
            self.metadata_blocks.as_mut_ptr(),
            self.metadata_blocks.len() as u32,
        ) {
            return Err(EncoderError::FailedToSetMetadata);
        }

        Ok(encoder)
    }

    unsafe fn cleanup(&mut self) {
        for block in self.metadata_blocks.iter() {
            FLAC__metadata_object_delete(*block);
        }
        self.metadata_blocks.clear();
    }

    pub fn build(mut self) -> Result<Vec<u8>, EncoderError> {
        unsafe {
            let encoder = self.prepare()?;
            let mut input_data: Vec<FLAC__int32> = Vec::with_capacity(self.total_samples());

            let mut samples_count = None;

            for channel in self.data {
                if samples_count.is_none() {
                    samples_count = Some(channel.len());
                } else if samples_count.unwrap() != channel.len() {
                    self.cleanup();
                    return Err(EncoderError::MismatchedSampleCountPerChannels);
                }
            }

            for sample_i in 0..samples_count.unwrap() {
                for channel_i in 0..self.data.len() {
                    input_data.push(self.data[channel_i][sample_i].to_bps_level(self.bps));
                }
            }

            let mut output_data =
                Vec::with_capacity(samples_count.unwrap_or(0) * self.data.len() as usize);
            let mut cursor = 0;

            let callback_data = WriteCallbackData {
                data: &mut output_data,
                cursor: &mut cursor,
            };

            if let Some(path) = self.output_path.clone() {
                FLAC__stream_encoder_init_file(encoder, path.as_bytes().as_ptr() as *const _, None, null_mut());
            } else {
                FLAC__stream_encoder_init_stream(
                    encoder,
                    Some(write_callback),
                    Some(seek_callback),
                    Some(tell_callback),
                    None,
                    &callback_data as *const _ as *mut _,
                );
            }

            let mut ok = 0;

            let channels = self.data.len();

            let block_size: usize = 1024 * channels;
            let mut input_cursor = 0;

            while input_cursor < input_data.len() {
                let remaining = input_data.len() - input_cursor;
                let used_block_size = block_size.min(remaining);

                let block = &input_data[input_cursor..];
                let block_ptr = block.as_ptr() as *const FLAC__int32;

                ok |= FLAC__stream_encoder_process_interleaved(
                    encoder,
                    block_ptr,
                    (used_block_size / channels) as u32,
                );

                input_cursor += block_size;
            }

            ok |= FLAC__stream_encoder_finish(encoder);

            self.cleanup();

            if ok == 0 {
                return Err(EncoderError::EncodingError);
            }

            Ok(output_data)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BpsLevel {
    Bps16,
    Bps20,
    Bps24,
}

impl BpsLevel {
    pub fn to_u32(&self) -> u32 {
        match self {
            BpsLevel::Bps16 => 16,
            BpsLevel::Bps20 => 20,
            BpsLevel::Bps24 => 24,
        }
    }
}

pub struct WriteCallbackData<'a> {
    pub data: &'a mut Vec<u8>,
    pub cursor: &'a mut usize,
}

#[no_mangle]
unsafe extern "C" fn write_callback(
    _encoder: *const FLAC__StreamEncoder,
    buffer: *const FLAC__byte,
    bytes: usize,
    _samples: u32,
    _current_frame: u32,
    client_data: *mut std::ffi::c_void,
) -> u32 {
    let data = unsafe { &mut *(client_data as *mut WriteCallbackData) };

    if *data.cursor + bytes > data.data.len() {
        let needed = (*data.cursor + bytes) - data.data.len();
        data.data.extend(vec![0u8; needed]);
    }

    let new_data = from_raw_parts(buffer, bytes);

    for i in 0..bytes {
        data.data[*data.cursor] = new_data[i];
        *data.cursor += 1;
    }

    0
}

#[no_mangle]
unsafe extern "C" fn seek_callback(
    _encoder: *const FLAC__StreamEncoder,
    absolute_byte_offset: u64,
    client_data: *mut std::ffi::c_void,
) -> u32 {
    let data = unsafe { &mut *(client_data as *mut WriteCallbackData) };

    *data.cursor = absolute_byte_offset as usize;

    FLAC__STREAM_ENCODER_SEEK_STATUS_OK
}

#[no_mangle]
unsafe extern "C" fn tell_callback(
    _encoder: *const FLAC__StreamEncoder,
    absolute_byte_offset: *mut u64,
    client_data: *mut std::ffi::c_void,
) -> u32 {
    let data = unsafe { &mut *(client_data as *mut WriteCallbackData) };

    *absolute_byte_offset = *data.cursor as u64;

    FLAC__STREAM_ENCODER_SEEK_STATUS_OK
}

#[derive(Debug)]
pub enum EncoderError {
    NoData,
    InitializationError,
    VerificationError,
    InvalidCompressionLevel,
    InvalidChannelCount,
    InvalidSampleType,
    TooManyOrTooFewSamples,
    MismatchedSampleCountPerChannels,
    FailedToInitializeEncoder,
    InvalidVorbisComment(String),
    FailedToSetMetadata,
    EncodingError,
    InvalidSampleRate,
}

pub trait IntoSample {
    fn to_i16(&self) -> i16;
    fn to_i20(&self) -> i32;
    fn to_i24(&self) -> i32;

    fn to_bps_level(&self, bps: BpsLevel) -> FLAC__int32 {
        match bps {
            BpsLevel::Bps16 => self.to_i16() as FLAC__int32,
            BpsLevel::Bps20 => self.to_i20(),
            BpsLevel::Bps24 => self.to_i24(),
        }
    }
}

impl IntoSample for f32 {
    fn to_i16(&self) -> i16 {
        let max = (1 << 15) - 1;
        (self.clamp(-1.0, 1.0) * max as f32) as i16
    }

    fn to_i20(&self) -> i32 {
        let max = (1 << 19) - 1;
        ((self.clamp(-1.0, 1.0) * max as f32) as i32).clamp(-max, max)
    }

    fn to_i24(&self) -> i32 {
        let max = (1 << 23) - 1;
        ((self.clamp(-1.0, 1.0) * max as f32) as i32).clamp(-max, max)
    }
}

impl IntoSample for f64 {
    fn to_i16(&self) -> i16 {
        let max = (1 << 15) - 1;
        (self.clamp(-1.0, 1.0) * max as f64) as i16
    }

    fn to_i20(&self) -> i32 {
        let max = (1 << 19) - 1;
        ((self.clamp(-1.0, 1.0) * max as f64) as i32).clamp(-max, max)
    }

    fn to_i24(&self) -> i32 {
        let max = (1 << 23) - 1;
        ((self.clamp(-1.0, 1.0) * max as f64) as i32).clamp(-max, max)
    }
}
