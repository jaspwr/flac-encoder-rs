use std::{
    ffi::c_char,
    mem::zeroed,
    ptr::null_mut, slice::from_raw_parts,
};

use libflac_sys::*;

pub struct FlacBuilder<'data, Sample>
    where Sample : IntoSample
{
    data: &'data Vec<Vec<Sample>>,
    bps: u32,
    sample_rate: u32,
    compression_level: u32,
    padding: u32,
    vorbis_commenets: Vec<(String, String)>,
}

impl<'data, Sample : IntoSample> FlacBuilder<'data, Sample> {
    pub fn new(data: &'data Vec<Vec<Sample>>, sample_rate: u32) -> Self {
        FlacBuilder {
            data,
            sample_rate,
            bps: 16,
            compression_level: 5,
            padding: 500,
            vorbis_commenets: vec![],
        }
    }

    pub fn compression_level(mut self, level: u32) -> Self {
        self.compression_level = level;
        self
    }

    pub fn padding(mut self, padding: u32) -> Self {
        self.padding = padding;
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
        if self.data.is_empty() {
            return Err(EncoderError::NoData);            
        }

        unsafe {
            if self.bps != 16 && self.bps != 20 && self.bps != 24 {
                return Err(EncoderError::InvalidBps);
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

            if 0 == FLAC__stream_encoder_set_bits_per_sample(encoder, self.bps) {
                return Err(EncoderError::InvalidSampleType);
            }

            if 0 == FLAC__stream_encoder_set_sample_rate(encoder, self.sample_rate) {
                return Err(EncoderError::InvalidSampleRate);
            }

            let total_samples = self
                .data
                .iter()
                .map(|channel| channel.len() as u64)
                .sum::<u64>();

            if 0 == FLAC__stream_encoder_set_total_samples_estimate(encoder, total_samples) {
                return Err(EncoderError::TooManyOrTooFewSamples);
            }

            // Metadata

            if self.vorbis_commenets.is_empty() {
                if 0 == FLAC__stream_encoder_set_metadata(encoder, null_mut(), 0) {
                    return Err(EncoderError::FailedToSetMetadata);
                }
            }

            let mut metadata_block = null_mut();
            let mut padding_block = FLAC__metadata_object_new(FLAC__METADATA_TYPE_PADDING);
            (*padding_block).length = padding;

            if !self.vorbis_commenets.is_empty() {
                metadata_block = FLAC__metadata_object_new(FLAC__METADATA_TYPE_VORBIS_COMMENT);

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

                    if 0 == FLAC__metadata_object_vorbiscomment_append_comment(
                        metadata_block,
                        entry,
                        0,
                    ) {
                        return Err(EncoderError::FailedToSetMetadata);
                    }
                }
            }

            if self.vorbis_commenets.is_empty() {
                let blocks = &mut padding_block;
                if 0 == FLAC__stream_encoder_set_metadata(encoder, blocks as *mut _, 1) {
                    return Err(EncoderError::FailedToSetMetadata);
                }
            } else {
                let mut blocks = vec![metadata_block, padding_block];
                if 0 == FLAC__stream_encoder_set_metadata(encoder, blocks.as_mut_ptr(), 2) {
                    return Err(EncoderError::FailedToSetMetadata);
                }
            }

            // Encoding

            // Interleaved, right aligned input data
            let mut input_data: Vec<FLAC__int32> = Vec::with_capacity(total_samples as usize);

            let mut samples_count = None;

            for channel in self.data {
                if samples_count.is_none() {
                    samples_count = Some(channel.len());
                } else if samples_count.unwrap() != channel.len() {
                    return Err(EncoderError::MismatchedSampleCountPerChannels);
                }
            }

            for sample_i in 0..samples_count.unwrap() {
                for channel_i in 0..self.data.len() {
                    input_data.push(self.data[channel_i][sample_i].to_i16() as FLAC__int32);
                    // input_data.push(1);
                }
            }

            let mut output_data = Vec::with_capacity(
                samples_count.unwrap_or(0) * self.data.len() as usize,
            );
            let mut cursor = 0;

            let callback_data = WriteCallbackData {
                data: &mut output_data,
                cursor: &mut cursor,
            };

            FLAC__stream_encoder_init_stream(
                encoder,
                Some(write_callback),
                Some(seek_callback),
                Some(tell_callback),
                None,
                &callback_data as *const _ as *mut _,
            );

            // FLAC__stream_encoder_init_file(encoder, b"C:/Users/jaspe/Downloads/aaaaa.flac\0".as_ptr() as *const _, None, null_mut());

            let mut ok = 0;

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

            if ok == 0 {
                return Err(EncoderError::EncodingError);
            }

            if !metadata_block.is_null() {
                FLAC__metadata_object_delete(metadata_block);
            }

            if !padding_block.is_null() {
                FLAC__metadata_object_delete(padding_block);
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
    InvalidBps,
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
