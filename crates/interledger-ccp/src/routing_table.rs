use crate::packet::{Route, RouteProp, RouteUpdateRequest};
use bytes::Bytes;
use hashbrown::HashMap;
use ring::rand::{SecureRandom, SystemRandom};

lazy_static! {
    static ref RANDOM: SystemRandom = SystemRandom::new();
}

struct PrefixMap<T> {
    map: HashMap<Bytes, T>,
    // TODO keep prefixes sorted
    // TODO use parallel iterator to find things in it
    prefixes: Vec<Bytes>,
}

impl<T> PrefixMap<T> {
    pub fn new() -> Self {
        PrefixMap {
            map: HashMap::new(),
            prefixes: Vec::new(),
        }
    }

    pub fn insert(&mut self, prefix: Bytes, item: T) -> bool {
        if self.map.insert(prefix.clone(), item).is_none() {
            self.prefixes.push(prefix);
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, prefix: Bytes) -> bool {
        if self.map.remove(&prefix).is_some() {
            if let Some(index) = self.prefixes.iter().position(|p| p == &prefix) {
                self.prefixes.remove(index);
            }
            true
        } else {
            false
        }
    }
}

pub struct RoutingTable {
    id: [u8; 16],
    epoch: u32,
    prefix_map: PrefixMap<Route>,
}

impl RoutingTable {
    pub fn new(id: [u8; 16]) -> Self {
        RoutingTable {
            id,
            epoch: 0,
            prefix_map: PrefixMap::new(),
        }
    }

    /// Remove the route for the given prefix. Returns true if that route existed before
    pub fn delete_route(&mut self, prefix: Bytes) -> bool {
        self.prefix_map.remove(prefix)
    }

    /// Add the given route. Returns true if that routed did not already exist
    pub fn add_route(&mut self, route: Route) -> bool {
        self.prefix_map.insert(route.prefix.clone(), route)
    }

    pub fn handle_update_request(
        &mut self,
        request: RouteUpdateRequest,
    ) -> Result<Vec<Bytes>, String> {
        if self.id != request.routing_table_id {
            debug!(
                "Saw new routing table. Old ID: {:x?}, new ID: {:x?}",
                self.id, request.routing_table_id
            );
            self.id = request.routing_table_id;
            self.epoch = 0;
        }

        if request.from_epoch_index > self.epoch {
            return Err(format!(
                "Gap in routing table. Expected epoch: {}, got from_epoch: {}",
                self.epoch, request.from_epoch_index
            ));
        }

        if request.to_epoch_index <= self.epoch {
            debug!(
                "Ignoring duplicate routing update for epoch: {}",
                self.epoch
            );
            return Ok(Vec::new());
        }

        if request.new_routes.is_empty() && request.withdrawn_routes.is_empty() {
            debug!(
                "Got heartbeat route update for table ID: {:x?}, epoch: {}",
                self.id, self.epoch
            );
            return Ok(Vec::new());
        }

        let mut changed_prefixes = Vec::new();
        for prefix in request.withdrawn_routes.iter() {
            if self.delete_route(prefix.clone()) {
                changed_prefixes.push(prefix.clone());
            }
        }

        for route in request.new_routes.into_iter() {
            let prefix = route.prefix.clone();
            if self.add_route(route) {
                changed_prefixes.push(prefix);
            }
        }

        Ok(changed_prefixes)
    }
}

impl Default for RoutingTable {
    fn default() -> RoutingTable {
        let mut id = [0; 16];
        RANDOM.fill(&mut id).expect("Unable to get randomness");
        RoutingTable::new(id)
    }
}

#[cfg(test)]
mod incoming_table {
    use super::*;
    use crate::fixtures::*;

    #[test]
    fn sets_id_if_update_has_different() {
        let mut table = RoutingTable::new([0; 16]);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 0;
        table.handle_update_request(request.clone()).unwrap();
        assert_eq!(table.id, request.routing_table_id);
        assert_eq!(table.epoch, 0);
    }

    #[test]
    fn errors_if_gap_in_epoch_indecies() {
        let mut table = RoutingTable::new([0; 16]);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 1;
        let result = table.handle_update_request(request);
        assert_eq!(
            result.unwrap_err(),
            "Gap in routing table. Expected epoch: 0, got from_epoch: 1"
        );
    }

    #[test]
    fn ignores_old_update() {
        let mut table = RoutingTable::new(UPDATE_REQUEST_COMPLEX.routing_table_id);
        table.epoch = 3;
        let mut request = UPDATE_REQUEST_COMPLEX.clone();
        request.from_epoch_index = 0;
        request.to_epoch_index = 1;
        let updated_routes = table.handle_update_request(request).unwrap();
        assert_eq!(updated_routes.len(), 0);
    }

    #[test]
    fn ignores_empty_update() {
        let mut table = RoutingTable::new([0; 16]);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 0;
        request.to_epoch_index = 1;
        let updated_routes = table.handle_update_request(request).unwrap();
        assert_eq!(updated_routes.len(), 0);
    }
}
