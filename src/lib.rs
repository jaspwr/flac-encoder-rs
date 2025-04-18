use std::{
    ffi::c_char,
    mem::zeroed,
    ptr::null_mut,
};

use libflac_sys::*;

pub struct FlacBuilder<'data, Sample> {
    pub data: &'data Vec<Vec<Sample>>,
    pub sample_rate: u32,
    pub compression_level: u32,
    pub vorbis_commenets: Vec<(String, String)>,
}

impl<'data, Sample> FlacBuilder<'data, Sample> {
    pub fn new(data: &'data Vec<Vec<Sample>>, sample_rate: u32) -> Self {
        FlacBuilder {
            data,
            sample_rate,
            compression_level: 5,
            vorbis_commenets: vec![],
        }
    }

    pub fn compression_level(mut self, level: u32) -> Self {
        self.compression_level = level;
        self
    }

    pub fn artist(self, artist: &str) -> Self {
        self.set_vorbis_comment("ARTIST", artist)
    }

    pub fn year(self, year: u32) -> Self {
        self.set_vorbis_comment("YEAR", &year.to_string())
    }

    pub fn set_vorbis_comment(mut self, key: &str, value: &str) -> Self {
        self.vorbis_commenets
            .push((format!("{}\0", key), format!("{}\0", value)));
        self
    }

    pub fn build(&self) -> Result<Vec<u8>, EncoderError> {
        unsafe {
            let encoder = FLAC__stream_encoder_new();

            if encoder.is_null() {
                return Err(EncoderError::InitializationError);
            }

            if 0 != FLAC__stream_encoder_set_verify(encoder, 1) {
                return Err(EncoderError::InvalidCompressionLevel);
            }

            if 0 != FLAC__stream_encoder_set_compression_level(encoder, self.compression_level) {
                return Err(EncoderError::InvalidCompressionLevel);
            }

            if 0 != FLAC__stream_encoder_set_channels(encoder, self.data.len() as u32) {
                return Err(EncoderError::InvalidChannelCount);
            }

            let bps = size_of::<Sample>() as u32 * 8;

            if 0 != FLAC__stream_encoder_set_bits_per_sample(encoder, bps) {
                return Err(EncoderError::InvalidSampleType);
            }

            let total_samples = self
                .data
                .iter()
                .map(|channel| channel.len() as u64)
                .sum::<u64>();

            if 0 != FLAC__stream_encoder_set_total_samples_estimate(encoder, total_samples) {
                return Err(EncoderError::TooManyOrTooFewSamples);
            }

            // Metadata

            if self.vorbis_commenets.is_empty() {
                if 0 != FLAC__stream_encoder_set_metadata(encoder, null_mut(), 0) {
                    return Err(EncoderError::FailedToSetMetadata);
                }
            }

            let mut metadata_block = null_mut();

            if !self.vorbis_commenets.is_empty() {
                metadata_block = FLAC__metadata_object_new(FLAC__METADATA_TYPE_VORBIS_COMMENT);

                if metadata_block.is_null() {
                    return Err(EncoderError::InitializationError);
                }

                for (key, value) in &self.vorbis_commenets {
                    let mut entry: FLAC__StreamMetadata_VorbisComment_Entry = zeroed();

                    if 0 != FLAC__metadata_object_vorbiscomment_entry_from_name_value_pair(
                        &mut entry,
                        key.as_bytes().as_ptr() as *const c_char,
                        value.as_bytes().as_ptr() as *const c_char,
                    ) {
                        return Err(EncoderError::InvalidVorbisComment(key.clone()));
                    }

                    if 0 != FLAC__metadata_object_vorbiscomment_append_comment(
                        metadata_block,
                        entry,
                        0,
                    ) {
                        return Err(EncoderError::FailedToSetMetadata);
                    }
                }

                let blocks = &mut metadata_block;
                if 0 != FLAC__stream_encoder_set_metadata(encoder, blocks as *mut _, 1) {
                    return Err(EncoderError::FailedToSetMetadata);
                }
            }

            // Encoding

            let mut input_data = vec![];
            let mut samples_count = None;
            for channel in self.data {
                input_data.push(channel.as_ptr() as *const FLAC__int32);

                if samples_count.is_none() {
                    samples_count = Some(channel.len());
                } else if samples_count.unwrap() != channel.len() {
                    return Err(EncoderError::MismatchedSampleCountPerChannels);
                }
            }

            let mut output_data = Vec::with_capacity(
                samples_count.unwrap_or(0) * self.data.len() as usize * size_of::<Sample>(),
            );
            let mut cursor = 0;

            let callback_data = WriteCallbackData {
                data: &mut output_data,
                cursor: &mut cursor,
            };

            let init_status = FLAC__stream_encoder_init_stream(
                encoder,
                Some(write_callback),
                None,
                None,
                None,
                &callback_data as *const _ as *mut _,
            );

            let mut ok = 0;

            ok |= FLAC__stream_encoder_process(
                encoder,
                self.data.as_ptr() as *const _,
                samples_count.unwrap_or(0) as u32,
            );
            ok |= FLAC__stream_encoder_finish(encoder);

            if ok != 0 {
                return Err(EncoderError::EncodingError);
            }

            if !metadata_block.is_null() {
                FLAC__metadata_object_delete(metadata_block);
            }

            Ok(output_data)
        }
    }
}

pub struct WriteCallbackData<'a> {
    pub data: &'a mut Vec<u8>,
    pub cursor: &'a mut usize,
}

#[no_mangle]
unsafe extern "C" fn write_callback(
    encoder: *const FLAC__StreamEncoder,
    buffer: *const FLAC__byte,
    bytes: usize,
    samples: u32,
    current_frame: u32,
    client_data: *mut std::ffi::c_void,
) -> u32 {
    let data = unsafe { &mut *(client_data as *mut WriteCallbackData) };

    data.data
        .extend_from_slice(std::slice::from_raw_parts(buffer, bytes));
    *data.cursor += bytes as usize;

    0
}

#[no_mangle]
unsafe extern "C" fn seek_callback(
    encoder: *const FLAC__StreamEncoder,
    absolute_byte_offset: u64,
    client_data: *mut std::ffi::c_void,
) -> u32 {
    let data = unsafe { &mut *(client_data as *mut WriteCallbackData) };

    0
}

#[derive(Debug)]
pub enum EncoderError {
    InitializationError,
    InvalidCompressionLevel,
    InvalidChannelCount,
    InvalidSampleType,
    TooManyOrTooFewSamples,
    MismatchedSampleCountPerChannels,
    FailedToInitializeEncoder,
    InvalidVorbisComment(String),
    FailedToSetMetadata,
    EncodingError,
}

