use std::{
    error::Error,
    fmt,
    sync::{Arc, RwLock},
    time::Duration,
};

use iroh::address_lookup::EndpointData;
use ma2a_core::{AddressEndpointDataV1, ProtocolError};
use tokio::{sync::watch, time::timeout};

use crate::address_endpoint_data_from_iroh;

#[derive(Clone, Debug)]
pub(crate) struct AddressObservation {
    state: Arc<RwLock<ObservationState>>,
    generation: watch::Sender<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ObservationValue {
    Unavailable,
    Ready(AddressEndpointDataV1),
    Invalid(ProtocolError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservationState {
    generation: u64,
    value: ObservationValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddressObservationError {
    Unavailable,
    Invalid(ProtocolError),
    Poisoned,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddressObservationWaitError {
    Observation(AddressObservationError),
    TimedOut,
}

impl fmt::Display for AddressObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("live address observation is unavailable"),
            Self::Invalid(error) => {
                write!(formatter, "live address observation is invalid: {error}")
            }
            Self::Poisoned => formatter.write_str("live address observation state is poisoned"),
            Self::Closed => formatter.write_str("live address observation notification closed"),
        }
    }
}

impl Error for AddressObservationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Unavailable | Self::Poisoned | Self::Closed => None,
        }
    }
}

impl fmt::Display for AddressObservationWaitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Observation(error) => error.fmt(formatter),
            Self::TimedOut => {
                formatter.write_str("live address observation initialization timed out")
            }
        }
    }
}

impl Error for AddressObservationWaitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Observation(error) => Some(error),
            Self::TimedOut => None,
        }
    }
}

impl Default for AddressObservation {
    fn default() -> Self {
        let (generation, _) = watch::channel(0);
        Self {
            state: Arc::new(RwLock::new(ObservationState {
                generation: 0,
                value: ObservationValue::Unavailable,
            })),
            generation,
        }
    }
}

impl AddressObservation {
    pub(crate) fn observe(&self, data: &EndpointData) {
        let value = match address_endpoint_data_from_iroh(data) {
            Ok(data) => ObservationValue::Ready(data),
            Err(error) => ObservationValue::Invalid(error),
        };
        let Ok(mut state) = self.state.write() else {
            return;
        };
        if state.value == value {
            return;
        }
        state.generation = state.generation.saturating_add(1);
        state.value = value;
        let generation = state.generation;
        drop(state);
        self.generation.send_replace(generation);
    }

    pub(crate) fn current(&self) -> Result<(u64, AddressEndpointDataV1), AddressObservationError> {
        let state = self
            .state
            .read()
            .map_err(|_| AddressObservationError::Poisoned)?;
        match &state.value {
            ObservationValue::Unavailable => Err(AddressObservationError::Unavailable),
            ObservationValue::Ready(data) => Ok((state.generation, data.clone())),
            ObservationValue::Invalid(error) => Err(AddressObservationError::Invalid(*error)),
        }
    }

    pub(crate) async fn wait_for_initial(
        &self,
        timeout_after: Duration,
    ) -> Result<(), AddressObservationWaitError> {
        let result = timeout(timeout_after, async {
            let mut receiver = self.generation.subscribe();
            loop {
                match self.current() {
                    Ok(_) => return Ok(()),
                    Err(AddressObservationError::Unavailable) => {}
                    Err(error) => return Err(error),
                }
                receiver
                    .changed()
                    .await
                    .map_err(|_| AddressObservationError::Closed)?;
            }
        })
        .await
        .map_err(|_| AddressObservationWaitError::TimedOut)?;
        result.map_err(AddressObservationWaitError::Observation)
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use iroh::address_lookup::{EndpointData, UserData};
    use iroh_base::{CustomAddr, TransportAddr};
    use ma2a_core::{MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN, ProtocolError};

    use super::{AddressObservation, AddressObservationError, AddressObservationWaitError};

    #[test]
    fn oversized_custom_data_sets_invalid_without_retaining_signable_data() {
        // Given
        let observation = AddressObservation::default();
        let oversized = vec![0x5a; MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN + 1];
        let data = EndpointData::new(vec![TransportAddr::Custom(CustomAddr::from_parts(
            9, &oversized,
        ))]);

        // When
        observation.observe(&data);

        // Then
        assert_eq!(
            observation.current(),
            Err(AddressObservationError::Invalid(
                ProtocolError::INVALID_INPUT
            ))
        );
    }

    #[test]
    fn seventeen_addresses_set_invalid_without_retaining_signable_data() {
        // Given
        let observation = AddressObservation::default();
        observation.observe(&EndpointData::new(vec![TransportAddr::Ip(
            std::net::SocketAddr::from(([127, 0, 0, 1], 4_899)),
        )]));
        assert!(observation.current().is_ok());
        let addresses = (0_u16..17)
            .map(|offset| {
                TransportAddr::Ip(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    4_900_u16 + offset,
                )))
            })
            .collect();

        // When
        observation.observe(&EndpointData::new(addresses));

        // Then
        assert_eq!(
            observation.current(),
            Err(AddressObservationError::Invalid(
                ProtocolError::INVALID_INPUT
            ))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn initial_wait_times_out_while_observation_remains_unavailable() {
        // Given
        let observation = AddressObservation::default();

        // When
        let result = observation.wait_for_initial(Duration::from_secs(2)).await;

        // Then
        assert_eq!(result, Err(AddressObservationWaitError::TimedOut));
        assert_eq!(
            observation.current(),
            Err(AddressObservationError::Unavailable)
        );
    }

    #[test]
    fn unavailable_and_poisoned_states_fail_closed() {
        // Given
        let observation = AddressObservation::default();
        let poisoned = observation.clone();

        // When
        assert_eq!(
            observation.current(),
            Err(AddressObservationError::Unavailable)
        );
        let state = Arc::clone(&poisoned.state);
        let _result = std::panic::catch_unwind(move || {
            let _guard = state.write().ok();
            std::panic::resume_unwind(Box::new("poison observation state"));
        });

        // Then
        assert_eq!(poisoned.current(), Err(AddressObservationError::Poisoned));
    }

    #[tokio::test]
    async fn generation_notifies_changes_without_queueing_observations()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Given
        let observation = AddressObservation::default();
        let mut generation = observation.subscribe();
        let addresses = vec![
            TransportAddr::Custom(CustomAddr::from_parts(31, b"first")),
            TransportAddr::Ip("127.0.0.1:4801".parse()?),
            TransportAddr::Custom(CustomAddr::from_parts(32, b"last")),
        ];
        let initial = EndpointData::new(addresses.clone());
        let updated = initial
            .clone()
            .with_user_data(UserData::try_from(String::new())?);

        // When
        observation.observe(&initial);
        generation.changed().await?;
        let initial_generation = *generation.borrow_and_update();
        observation.observe(&updated);
        generation.changed().await?;
        let updated_generation = *generation.borrow_and_update();
        observation.observe(&updated);

        // Then
        assert_eq!(initial_generation, 1);
        assert_eq!(updated_generation, 2);
        assert!(!generation.has_changed()?);
        assert_eq!(
            observation
                .current()
                .map(|(_, data)| data.addresses().to_vec()),
            Ok(addresses)
        );
        assert_eq!(
            observation
                .current()
                .map(|(_, data)| data.user_data().map(str::to_owned)),
            Ok(Some(String::new()))
        );
        Ok(())
    }
}
