use iroh_base::SecretKey;
use ma2a_net::{LocalIrohRelayMap, PublicRelayFallbackConfig};
use ma2a_store::Repository;

use super::harness::{
    NOW_MS, RelayFixture, SpaceFixture, TempState, TestValue, create_space, store_relay,
};

pub(super) struct RelayMatrix {
    _state: TempState,
    repository: Repository,
    pub(super) local: SecretKey,
}

impl RelayMatrix {
    pub(super) fn new(name: &str) -> TestValue<Self> {
        let state = TempState::new(name)?;
        let local = SecretKey::from_bytes(&[0x74; 32]);
        let relay_x = SecretKey::from_bytes(&[0x75; 32]);
        let relay_y = SecretKey::from_bytes(&[0x76; 32]);
        let common = SecretKey::from_bytes(&[0x77; 32]);
        let mut repository = Repository::open(&state.config())?;
        let x = create_space(
            &mut repository,
            SpaceFixture {
                local: &local,
                relays: &[&relay_x, &common],
                issued_at_ms: 20,
            },
        )?;
        let y = create_space(
            &mut repository,
            SpaceFixture {
                local: &local,
                relays: &[&relay_y, &common],
                issued_at_ms: 30,
            },
        )?;
        for fixture in [
            RelayFixture {
                authorization: &x,
                relay: &relay_x,
                relay_url: "https://rx.example.invalid".parse()?,
                sequence: 1,
            },
            RelayFixture {
                authorization: &y,
                relay: &relay_y,
                relay_url: "https://ry.example.invalid".parse()?,
                sequence: 1,
            },
        ] {
            store_relay(&mut repository, fixture)?;
        }
        Ok(Self {
            _state: state,
            repository,
            local,
        })
    }

    pub(super) fn map(
        &self,
        fallback: Option<&PublicRelayFallbackConfig>,
    ) -> TestValue<LocalIrohRelayMap> {
        let spaces = self
            .repository
            .control_spaces_for(self.local.public().into())?;
        Ok(LocalIrohRelayMap::from_control_spaces(
            &spaces, fallback, NOW_MS,
        ))
    }
}
