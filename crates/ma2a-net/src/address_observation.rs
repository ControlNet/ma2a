use std::{
    error::Error,
    fmt,
    sync::{Arc, RwLock},
};

use iroh::address_lookup::EndpointData;
use ma2a_core::{AddressEndpointDataV1, ProtocolError};
use tokio::sync::watch;

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

    pub(crate) async fn wait_for_initial(&self) -> Result<(), AddressObservationError> {
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
    }

    #[cfg(test)]
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iroh::address_lookup::{EndpointData, UserData};
    use iroh_base::{CustomAddr, TransportAddr};
    use ma2a_core::{MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN, ProtocolError};

    use super::{AddressObservation, AddressObservationError};

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
        let initial = EndpointData::new(vec![TransportAddr::Ip("127.0.0.1:4801".parse()?)]);
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

        // Then
        assert_eq!(initial_generation, 1);
        assert_eq!(updated_generation, 2);
        assert_eq!(
            observation
                .current()
                .map(|(_, data)| data.user_data().map(str::to_owned)),
            Ok(Some(String::new()))
        );
        Ok(())
    }
}
