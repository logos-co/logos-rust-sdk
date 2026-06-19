//! The SensorModule contract, authored in Rust.

/// A sensor hub exposing typed readings, batch queries, and status events.
pub trait SensorModule {
    /// Returns the latest temperature reading in degrees Celsius.
    fn temperature(&mut self) -> f64;

    /// Enables or disables the sensor.
    /// Returns the new enabled state.
    fn enable(&mut self, on: bool) -> bool;

    /// Renames the sensor channel.
    fn rename(&mut self, id: u64, name: String) -> String;

    /// Calibrates a channel with an offset and a human-readable label.
    fn calibrate(&mut self, id: u64, offset: f64, label: String) -> bool;

    /// Records a reading and returns the new sample count.
    fn record(&mut self, id: u64, value: f64, note: String, valid: bool) -> i64;

    /// Flashes raw firmware bytes and echoes back the stored image.
    fn firmware(&mut self, image: Vec<u8>) -> Vec<u8>;

    /// Resolves a batch of channel ids to their labels.
    fn labels(&mut self, ids: Vec<u64>) -> Vec<String>;

    /// Computes the mean of a batch of samples.
    fn average(&mut self, samples: Vec<f64>) -> f64;

    /// Resets a channel; returns a structured success/error result.
    fn reset(&mut self, id: String) -> Result<serde_json::Value, String>;

    /// Framework hook — defaulted, so NOT part of the contract.
    fn on_context_ready(&mut self) {}
}

/// Events are declared on a companion `<Trait>Events` trait.
pub trait SensorModuleEvents {
    /// Fires once the sensor has finished warming up.
    fn ready(&self);

    /// Fires on each new reading with the channel id and value.
    fn reading(&self, id: u64, value: f64);

    /// Fires when a channel faults.
    /// Carries an error code, a message, and whether the fault is fatal.
    fn fault(&self, code: i64, message: String, fatal: bool);
}
