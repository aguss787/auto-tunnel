use backoff::SystemClock;

pub struct Backoff {
    timer: backoff::exponential::ExponentialBackoff<SystemClock>,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            timer: backoff::exponential::ExponentialBackoffBuilder::new()
                .with_initial_interval(std::time::Duration::from_micros(1))
                .with_max_interval(std::time::Duration::from_millis(500))
                .with_multiplier(1.1)
                .with_max_elapsed_time(None)
                .build(),
        }
    }

    pub fn reset(&mut self) {
        backoff::backoff::Backoff::reset(&mut self.timer);
    }

    pub fn sleep(&mut self) {
        let duration = backoff::backoff::Backoff::next_backoff(&mut self.timer)
            .expect("sleep timer return no backoff time");

        std::thread::sleep(duration);
    }
}
