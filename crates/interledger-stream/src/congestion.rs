#[cfg(feature = "metrics_csv")]
use chrono::Utc;
#[cfg(feature = "metrics_csv")]
use csv;
use interledger_packet::{ErrorCode, MaxPacketAmountDetails, Reject};
#[cfg(test)]
use lazy_static::lazy_static;
use log::{debug, warn};
use std::cmp::{max, min};
#[cfg(feature = "metrics_csv")]
use std::io;

/// A basic congestion controller that implements an
/// Additive Increase, Multiplicative Decrease (AIMD) algorithm.
///
/// Future implementations of this will use more advanced congestion
/// control algorithms.
pub struct CongestionController {
    state: CongestionState,
    increase_amount: u64,
    decrease_factor: f64,
    max_packet_amount: Option<u64>,
    amount_in_flight: u64,
    max_in_flight: u64,
    #[cfg(feature = "metrics_csv")]
    csv_writer: csv::Writer<io::Stdout>,
}

#[derive(PartialEq)]
enum CongestionState {
    SlowStart,
    AvoidCongestion,
}

impl CongestionController {
    pub fn new(start_amount: u64, increase_amount: u64, decrease_factor: f64) -> Self {
        #[cfg(feature = "metrics_csv")]
        let mut csv_writer = csv::Writer::from_writer(io::stdout());
        #[cfg(feature = "metrics_csv")]
        csv_writer
            .write_record(&["time", "max_amount_in_flight", "amount_fulfilled"])
            .unwrap();

        CongestionController {
            state: CongestionState::SlowStart,
            increase_amount,
            decrease_factor,
            max_packet_amount: None,
            amount_in_flight: 0,
            max_in_flight: start_amount,
            #[cfg(feature = "metrics_csv")]
            csv_writer,
        }
    }

    pub fn get_max_amount(&mut self) -> u64 {
        let amount_left_in_window = self.max_in_flight - self.amount_in_flight;
        if let Some(max_packet_amount) = self.max_packet_amount {
            min(amount_left_in_window, max_packet_amount)
        } else {
            amount_left_in_window
        }
    }

    pub fn prepare(&mut self, amount: u64) {
        if amount > 0 {
            self.amount_in_flight += amount;
            debug!(
                "Prepare packet of {}, amount in flight is now: {}",
                amount, self.amount_in_flight
            );
        }
    }

    pub fn fulfill(&mut self, prepare_amount: u64) {
        self.amount_in_flight -= prepare_amount;

        // Before we know how much we should be sending at a time,
        // double the window size on every successful packet.
        // Once we start getting errors, switch to Additive Increase,
        // Multiplicative Decrease (AIMD) congestion avosequenceance
        if self.state == CongestionState::SlowStart {
            // Double the max in flight but don't exceed the u64 max value
            if u64::max_value() / 2 >= self.max_in_flight {
                self.max_in_flight *= 2;
            } else {
                self.max_in_flight = u64::max_value();
            }
            debug!(
                "Fulfilled packet of {}, doubling max in flight to: {}",
                prepare_amount, self.max_in_flight
            );
        } else {
            // Add to the max in flight but don't exeed the u64 max value
            if u64::max_value() - self.increase_amount >= self.max_in_flight {
                self.max_in_flight += self.increase_amount;
            } else {
                self.max_in_flight = u64::max_value();
            }
            debug!(
                "Fulfilled packet of {}, increasing max in flight to: {}",
                prepare_amount, self.max_in_flight
            );
        }

        #[cfg(feature = "metrics_csv")]
        self.log_stats(prepare_amount);
    }

    pub fn reject(&mut self, prepare_amount: u64, reject: &Reject) {
        self.amount_in_flight -= prepare_amount;

        match reject.code() {
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY => {
                self.state = CongestionState::AvoidCongestion;
                self.max_in_flight = max(
                    (self.max_in_flight as f64 / self.decrease_factor).floor() as u64,
                    1,
                );
                debug!("Rejected packet with T04 error. Amount in flight was: {}, decreasing max in flight to: {}", self.amount_in_flight + prepare_amount, self.max_in_flight);

                #[cfg(feature = "metrics_csv")]
                self.log_stats(0);
            }
            ErrorCode::F08_AMOUNT_TOO_LARGE => {
                if let Ok(details) = MaxPacketAmountDetails::from_bytes(reject.data()) {
                    let new_max_packet_amount: u64 =
                        prepare_amount * details.max_amount() / details.amount_received();
                    if let Some(max_packet_amount) = self.max_packet_amount {
                        self.max_packet_amount =
                            Some(min(max_packet_amount, new_max_packet_amount));
                    } else {
                        self.max_packet_amount = Some(new_max_packet_amount);
                    }
                } else {
                    // TODO lower the max packet amount anyway
                    warn!("Got F08: Amount Too Large Error without max packet amount details attached");
                }
            }
            _ => {
                // No special treatment for other errors
            }
        }
    }

    #[cfg(test)]
    fn set_max_packet_amount(&mut self, max_packet_amount: u64) {
        self.max_packet_amount = Some(max_packet_amount)
    }

    #[cfg(feature = "metrics_csv")]
    fn log_stats(&mut self, amount_sent: u64) {
        self.csv_writer
            .write_record(&[
                format!("{}", Utc::now().timestamp_millis()),
                format!("{}", self.max_in_flight),
                format!("{}", amount_sent),
            ])
            .unwrap();
        self.csv_writer.flush().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod slow_start {
        use super::*;

        #[test]
        fn doubles_max_amount_on_fulfill() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 2000);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 4000);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 8000);
        }

        #[test]
        fn doesnt_overflow_u64() {
            let mut controller = CongestionController {
                state: CongestionState::SlowStart,
                increase_amount: 1000,
                decrease_factor: 2.0,
                max_packet_amount: None,
                amount_in_flight: 0,
                max_in_flight: u64::max_value() - 1,
                #[cfg(feature = "metrics_csv")]
                csv_writer: csv::Writer::from_writer(io::stdout()),
            };

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), u64::max_value());
        }
    }

    mod congestion_avoidance {
        use super::*;
        use interledger_packet::RejectBuilder;

        lazy_static! {
            static ref INSUFFICIENT_LIQUIDITY_ERROR: Reject = RejectBuilder {
                code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                message: &[],
                triggered_by: None,
                data: &[],
            }
            .build();
        }

        #[test]
        fn additive_increase() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.state = CongestionState::AvoidCongestion;
            for i in 1..5 {
                let amount = i * 1000;
                controller.prepare(amount);
                controller.fulfill(amount);
                assert_eq!(controller.get_max_amount(), 1000 + i * 1000);
            }
        }

        #[test]
        fn multiplicative_decrease() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.state = CongestionState::AvoidCongestion;

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.reject(amount, &*INSUFFICIENT_LIQUIDITY_ERROR);
            assert_eq!(controller.get_max_amount(), 500);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.reject(amount, &*INSUFFICIENT_LIQUIDITY_ERROR);
            assert_eq!(controller.get_max_amount(), 250);
        }

        #[test]
        fn aimd_combined() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.state = CongestionState::AvoidCongestion;

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 2000);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 3000);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.reject(amount, &*INSUFFICIENT_LIQUIDITY_ERROR);
            assert_eq!(controller.get_max_amount(), 1500);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 2500);
        }

        #[test]
        fn max_packet_amount() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            assert_eq!(controller.get_max_amount(), 1000);

            controller.prepare(1000);
            controller.reject(
                1000,
                &RejectBuilder {
                    code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                    message: &[],
                    triggered_by: None,
                    data: &MaxPacketAmountDetails::new(100, 10).to_bytes(),
                }
                .build(),
            );

            assert_eq!(controller.get_max_amount(), 100);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 100);

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), 100);
        }

        #[test]
        fn doesnt_overflow_u64() {
            let mut controller = CongestionController {
                state: CongestionState::AvoidCongestion,
                increase_amount: 1000,
                decrease_factor: 2.0,
                max_packet_amount: None,
                amount_in_flight: 0,
                max_in_flight: u64::max_value() - 1,
                #[cfg(feature = "metrics_csv")]
                csv_writer: csv::Writer::from_writer(io::stdout()),
            };

            let amount = controller.get_max_amount();
            controller.prepare(amount);
            controller.fulfill(amount);
            assert_eq!(controller.get_max_amount(), u64::max_value());
        }
    }

    mod tracking_amount_in_flight {
        use super::*;

        #[test]
        fn tracking_amount_in_flight() {
            let mut controller = CongestionController::new(1000, 1000, 2.0);
            controller.set_max_packet_amount(600);
            assert_eq!(controller.get_max_amount(), 600);

            controller.prepare(100);
            assert_eq!(controller.get_max_amount(), 600);

            controller.prepare(600);
            assert_eq!(controller.get_max_amount(), 1000 - 600 - 100);
        }
    }
}
