#![doc = include_str!("../README.md")]

use std::{
    ffi::{c_char, CString},
    mem::zeroed,
    os::raw::c_void,
    path::Path,
    ptr::null_mut,
    slice::from_raw_parts,
    str::FromStr,
};

use libflac_sys::*;

pub struct FlacBuilder<'data, Sample>
where
    Sample: IntoSample,
{
    data: InputData<'data, Sample>,
    bps: BpsLevel,
    sample_rate: u32,
    compression_level: u32,
    padding: u32,
    vorbis_comments: Vec<(CString, CString)>,
    metadata_blocks: Vec<*mut FLAC__StreamMetadata>,
}

impl<'data, Sample: IntoSample> FlacBuilder<'data, Sample> {
    /// New with planar audio data. The input data must be a list of channels where each channel is
    /// a list of frames/samples. Samples can be either `f32` or `f64` in range [-1.0, 1.0] or
    /// anything you implement `IntoSample` on.
    pub fn from_planar(data: &'data [Vec<Sample>], sample_rate: u32) -> Self {
        Self::new(InputData::Planar(data), sample_rate)
    }

    /// New with interleaved (e.g. LRLRLRLRLRLR) audio data. Samples can be either `f32` or `f64` 
    /// in range [-1.0, 1.0] or anything you implement `IntoSample` on.
    pub fn from_interleaved(data: &'data [Sample], channels: usize, sample_rate: u32) -> Self {
        Self::new(InputData::Interleaved { data, channels }, sample_rate)
    }

    fn new(data: InputData<'data, Sample>, sample_rate: u32) -> Self {
        FlacBuilder {
            data,
            sample_rate,
            bps: BpsLevel::Bps16,
            compression_level: 5,
            padding: 500,
            vorbis_comments: vec![],
            metadata_blocks: vec![],
        }
    }

    /// See [here](https://xiph.org/flac/api/group__flac__stream__encoder.html#gaacc01aab02849119f929b8516420fcd3).
    pub fn compression_level(mut self, level: u32) -> Self {
        self.compression_level = level;
        self
    }

    /// Set bits per sample.
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

    pub fn album(self, album: &str) -> Self {
        self.vorbis_comment("ALBUM", album)
    }

    pub fn title(self, title: &str) -> Self {
        self.vorbis_comment("TITLE", title)
    }

    pub fn year(self, year: u32) -> Self {
        self.vorbis_comment("YEAR", &year.to_string())
    }

    pub fn track_number(self, number: i32) -> Self {
        self.vorbis_comment("TRACKNUMBER", &number.to_string())
    }

    pub fn vorbis_comment(mut self, key: &str, value: &str) -> Self {
        self.vorbis_comments.push((
            CString::from_str(key).unwrap_or_default(),
            CString::from_str(value).unwrap_or_default(),
        ));
        self
    }

    unsafe fn prepare(&mut self) -> Result<*mut FLAC__StreamEncoder, EncoderError> {
        if !self.data.channel_sizes_match() {
            return Err(EncoderError::MismatchedSampleCountPerChannels);
        }

        if self.data.total_samples() == 0 {
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

        let channels = self.data.channel_count();

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
            self.data.total_samples() as u64,
        ) {
            return Err(EncoderError::TooManyOrTooFewSamples);
        }

        if self.vorbis_comments.is_empty() {
            if 0 == FLAC__stream_encoder_set_metadata(encoder, null_mut(), 0) {
                return Err(EncoderError::FailedToSetMetadata);
            }
        }

        if !self.vorbis_comments.is_empty() {
            let metadata_block = FLAC__metadata_object_new(FLAC__METADATA_TYPE_VORBIS_COMMENT);

            if metadata_block.is_null() {
                return Err(EncoderError::InitializationError);
            }

            for (key, value) in &self.vorbis_comments {
                let mut entry: FLAC__StreamMetadata_VorbisComment_Entry = zeroed();

                if 0 == FLAC__metadata_object_vorbiscomment_entry_from_name_value_pair(
                    &mut entry,
                    key.as_bytes().as_ptr() as *const c_char,
                    value.as_bytes().as_ptr() as *const c_char,
                ) {
                    return Err(EncoderError::InvalidVorbisComment(
                        key.to_string_lossy().to_string(),
                    ));
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

    pub fn write_file(mut self, path: impl AsRef<Path>) -> Result<(), EncoderError> {
        unsafe {
            let encoder = self.prepare()?;

            let Ok(path) = CString::from_str(&path.as_ref().to_string_lossy()) else {
                return Err(EncoderError::NullCharInPath);
            };

            FLAC__stream_encoder_init_file(
                encoder,
                path.as_bytes().as_ptr() as *const _,
                None,
                null_mut(),
            );

            self.feed_entire_input(encoder)?;

            finish(encoder)?;

            Ok(())
        }
    }

    pub fn build(mut self) -> Result<Vec<u8>, EncoderError> {
        unsafe {
            let encoder = self.prepare()?;

            let mut callback_data = WriteCallbackData {
                data: Vec::with_capacity(self.data.total_samples()),
                cursor: 0,
            };

            FLAC__stream_encoder_init_stream(
                encoder,
                Some(write_callback),
                Some(seek_callback),
                Some(tell_callback),
                None,
                &mut callback_data as *mut _ as *mut c_void,
            );

            self.feed_entire_input(encoder)?;

            finish(encoder)?;

            Ok(callback_data.data)
        }
    }

    fn feed_entire_input(&mut self, encoder: *mut FLAC__StreamEncoder) -> Result<(), EncoderError> {
        let mut input_cursor = 0;

        while input_cursor < self.data.samples_per_channel() {
            self.consume_input_chunk(encoder, &mut input_cursor, 1024)?;
        }

        Ok(())
    }

    fn consume_input_chunk(
        &mut self,
        encoder: *mut FLAC__StreamEncoder,
        input_cursor: &mut usize,
        chunk_size: usize,
    ) -> Result<(), EncoderError> {
        let channels = self.data.channel_count();

        let mut input_data: Vec<FLAC__int32> = Vec::with_capacity(chunk_size * channels);

        for block_sample_i in 0..chunk_size {
            for channel_i in 0..self.data.channel_count() {
                input_data.push(
                    match &self.data {
                        InputData::Interleaved { data, channels } => data
                            .get((*input_cursor + block_sample_i) * channels + channel_i)
                            .copied()
                            .unwrap_or(Sample::default()),
                        InputData::Planar(data) => data
                            .get(channel_i)
                            .and_then(|c| c.get(*input_cursor + block_sample_i))
                            .copied()
                            .unwrap_or(Sample::default()),
                    }
                    .to_bps_level(self.bps),
                );
            }
        }

        let remaining = self.data.total_samples() - *input_cursor;
        let actual_size = chunk_size.min(remaining);

        let block_ptr = input_data.as_ptr() as *const FLAC__int32;

        unsafe {
            if 0 == FLAC__stream_encoder_process_interleaved(encoder, block_ptr, actual_size as u32)
            {
                return Err(EncoderError::EncodingError);
            }
        }

        *input_cursor += chunk_size;

        Ok(())
    }

    unsafe fn cleanup(&mut self) {
        for block in self.metadata_blocks.iter() {
            FLAC__metadata_object_delete(*block);
        }
        self.metadata_blocks.clear();
    }
}

unsafe fn finish(encoder: *mut FLAC__StreamEncoder) -> Result<(), EncoderError> {
    if 0 == FLAC__stream_encoder_finish(encoder) {
        return Err(EncoderError::EncodingError);
    }

    Ok(())
}

impl<'data, Sample: IntoSample> Drop for FlacBuilder<'data, Sample> {
    fn drop(&mut self) {
        unsafe {
            self.cleanup();
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
    fn to_u32(&self) -> u32 {
        match self {
            BpsLevel::Bps16 => 16,
            BpsLevel::Bps20 => 20,
            BpsLevel::Bps24 => 24,
        }
    }
}

struct WriteCallbackData {
    data: Vec<u8>,
    cursor: usize,
}

enum InputData<'a, Sample>
where
    Sample: IntoSample,
{
    Interleaved { data: &'a [Sample], channels: usize },
    Planar(&'a [Vec<Sample>]),
}

impl<'a, Sample: IntoSample> InputData<'a, Sample> {
    fn channel_count(&self) -> usize {
        match self {
            InputData::Interleaved { channels, .. } => *channels,
            InputData::Planar(data) => data.len(),
        }
    }

    fn samples_per_channel(&self) -> usize {
        match self {
            InputData::Interleaved { data, channels } => data.len() / channels,
            InputData::Planar(data) => {
                if data.is_empty() {
                    return 0;
                }
                data[0].len()
            }
        }
    }

    fn total_samples(&self) -> usize {
        match self {
            InputData::Interleaved { data, .. } => data.len(),
            InputData::Planar(data) => data.iter().map(|channel| channel.len()).sum(),
        }
    }

    fn channel_sizes_match(&self) -> bool {
        match self {
            InputData::Interleaved { data, channels } => data.len() % *channels == 0,
            InputData::Planar(data) => {
                if data.is_empty() {
                    return true;
                }
                let size = data[0].len();
                data.iter().all(|channel| channel.len() == size)
            }
        }
    }
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

    if data.cursor + bytes > data.data.len() {
        let needed = (data.cursor + bytes) - data.data.len();
        data.data.extend(vec![0u8; needed]);
    }

    let new_data = from_raw_parts(buffer, bytes);

    for i in 0..bytes {
        data.data[data.cursor] = new_data[i];
        data.cursor += 1;
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

    data.cursor = absolute_byte_offset as usize;

    FLAC__STREAM_ENCODER_SEEK_STATUS_OK
}

#[no_mangle]
unsafe extern "C" fn tell_callback(
    _encoder: *const FLAC__StreamEncoder,
    absolute_byte_offset: *mut u64,
    client_data: *mut std::ffi::c_void,
) -> u32 {
    let data = unsafe { &mut *(client_data as *mut WriteCallbackData) };

    *absolute_byte_offset = data.cursor as u64;

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
    NullCharInPath,
}

/// `f32` and `f64` in `[-1.0, 1.0]`.
pub trait IntoSample: Copy + Default {
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
