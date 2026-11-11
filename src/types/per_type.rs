//! One value for the audio stream and one for the video stream.

use super::DataType;

#[derive(Debug, Clone, Copy, Default)]
pub struct PerType<T> {
    pub audio: T,
    pub video: T,
}

impl<T> PerType<T> {
    pub fn get(&self, data_type: DataType) -> &T {
        match data_type {
            DataType::Audio => &self.audio,
            DataType::Video => &self.video,
        }
    }

    pub fn get_mut(&mut self, data_type: DataType) -> &mut T {
        match data_type {
            DataType::Audio => &mut self.audio,
            DataType::Video => &mut self.video,
        }
    }
}
