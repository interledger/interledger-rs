use crate::packet::{Route, RouteUpdateRequest};
use hex;
use log::{debug, trace};
use once_cell::sync::Lazy;
use ring::rand::{SecureRandom, SystemRandom};
use std::collections::HashMap;
use std::iter::FromIterator;

static RANDOM: Lazy<SystemRandom> = Lazy::new(SystemRandom::new);

#[derive(Debug, Clone)]
struct PrefixMap<T> {
    map: HashMap<String, T>,
}

impl<T> PrefixMap<T> {
    pub fn new() -> Self {
        PrefixMap {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, prefix: String, item: T) -> bool {
        self.map.insert(prefix, item).is_none()
    }

    pub fn remove(&mut self, prefix: &str) -> bool {
        self.map.remove(prefix).is_some()
    }

    pub fn resolve(&self, prefix: &str) -> Option<&T> {
        // TODO use parallel iterator
        self.map
            .iter()
            .filter(|(p, _)| prefix.starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
            .map(|(_prefix, item)| item)
    }
}

/// The routing table is identified by an ID (a UUID in array form) and an "epoch".
/// When an Interledger node reloads, it will generate a new UUID for its routing table.
/// Each update applied increments the epoch number, so it acts as a version tracker.
/// This helps peers make sure they are in sync with one another and request updates if not.
#[derive(Debug, Clone)]
pub struct RoutingTable<A> {
    id: [u8; 16],
    epoch: u32,
    prefix_map: PrefixMap<(A, Route)>,
}

impl<A> RoutingTable<A>
where
    A: Clone,
{
    pub(crate) fn new(id: [u8; 16]) -> Self {
        RoutingTable {
            id,
            epoch: 0,
            prefix_map: PrefixMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_id(&mut self, id: [u8; 16]) {
        self.id = id;
        self.epoch = 0;
    }

    #[cfg(test)]
    pub(crate) fn set_epoch(&mut self, epoch: u32) {
        self.epoch = epoch;
    }

    pub(crate) fn id(&self) -> [u8; 16] {
        self.id
    }

    pub(crate) fn epoch(&self) -> u32 {
        self.epoch
    }

    pub(crate) fn increment_epoch(&mut self) -> u32 {
        let epoch = self.epoch;
        self.epoch += 1;
        epoch
    }

    /// Set a particular route, overwriting the one that was there before
    pub(crate) fn set_route(&mut self, prefix: String, account: A, route: Route) {
        self.prefix_map.remove(&prefix);
        self.prefix_map.insert(prefix, (account, route));
    }

    /// Remove the route for the given prefix. Returns true if that route existed before
    pub(crate) fn delete_route(&mut self, prefix: &str) -> bool {
        self.prefix_map.remove(prefix)
    }

    /// Add the given route. Returns true if that routed did not already exist
    pub(crate) fn add_route(&mut self, account: A, route: Route) -> bool {
        self.prefix_map
            .insert(route.prefix.clone(), (account, route))
    }

    /// Get the best route we have for the given prefix
    pub(crate) fn get_route(&self, prefix: &str) -> Option<&(A, Route)> {
        self.prefix_map.resolve(prefix)
    }

    pub(crate) fn get_simplified_table(&self) -> HashMap<String, A> {
        HashMap::from_iter(
            self.prefix_map
                .map
                .iter()
                .map(|(address, (account, _route))| (address.clone(), account.clone())),
        )
    }

    /// Handle a CCP Route Update Request from the peer this table represents
    pub(crate) fn handle_update_request(
        &mut self,
        account: A,
        request: RouteUpdateRequest,
    ) -> Result<Vec<String>, String> {
        if self.id != request.routing_table_id {
            debug!(
                "Saw new routing table. Old ID: {}, new ID: {}",
                hex::encode(&self.id[..]),
                hex::encode(&request.routing_table_id[..])
            );
            self.id = request.routing_table_id;
            self.epoch = 0;
        }

        if request.from_epoch_index > self.epoch {
            return Err(format!(
                "Gap in routing table {}. Expected epoch: {}, got from_epoch: {}",
                hex::encode(&self.id[..]),
                self.epoch,
                request.from_epoch_index
            ));
        }

        // It is OK to receive epochs with the same index as our current epoch
        if request.to_epoch_index < self.epoch {
            trace!(
                "Ignoring routing update from old epoch. Received epoch: {}. Our epoch: {}",
                request.to_epoch_index,
                self.epoch
            );
            return Ok(Vec::new());
        }

        // Update the table with the epoch, new routes, and
        // withdrawn routes received in the route update request
        self.epoch = request.to_epoch_index;

        if request.new_routes.is_empty() && request.withdrawn_routes.is_empty() {
            trace!(
                "Got heartbeat route update for table ID: {}, epoch: {}",
                hex::encode(&self.id[..]),
                self.epoch
            );
        }

        let mut changed_prefixes =
            Vec::with_capacity(request.new_routes.len() + request.withdrawn_routes.len());
        for prefix in request.withdrawn_routes.iter() {
            if self.delete_route(prefix) {
                changed_prefixes.push(prefix.to_string());
            }
        }

        for route in request.new_routes.into_iter() {
            let prefix = route.prefix.clone();
            if self.add_route(account.clone(), route) {
                changed_prefixes.push(prefix);
            }
        }

        trace!(
            "Updated routing table {} to epoch: {}",
            hex::encode(&self.id[..]),
            self.epoch
        );

        Ok(changed_prefixes)
    }
}

impl<A> Default for RoutingTable<A>
where
    A: Clone,
{
    fn default() -> RoutingTable<A> {
        let mut id = [0; 16];
        RANDOM.fill(&mut id).expect("Unable to get randomness");
        RoutingTable::new(id)
    }
}

#[cfg(test)]
mod prefix_map {
    use super::*;

    #[test]
    fn doesnt_insert_duplicates() {
        let mut map = PrefixMap::new();
        assert!(map.insert("example.a".to_string(), 1));
        assert!(!map.insert("example.a".to_string(), 1));
    }

    #[test]
    fn removes_entry() {
        let mut map = PrefixMap::new();
        assert!(map.insert("example.a".to_string(), 1));
        assert!(map.remove("example.a"));
        assert!(map.map.is_empty());
    }

    #[test]
    fn resolves_to_longest_matching_prefix() {
        let mut map = PrefixMap::new();
        map.insert("example.a".to_string(), 1);
        map.insert("example.a.b.c".to_string(), 2);
        map.insert("example.a.b".to_string(), 3);

        assert_eq!(map.resolve("example.a").unwrap(), &1);
        assert_eq!(map.resolve("example.a.b.c").unwrap(), &2);
        assert_eq!(map.resolve("example.a.b.c.d.e").unwrap(), &2);
        assert!(map.resolve("example.other").is_none());
    }
}

#[cfg(test)]
mod table {
    use super::*;
    use crate::fixtures::*;
    use crate::test_helpers::*;
    use uuid::Uuid;

    #[test]
    fn sets_id_if_update_has_different() {
        let mut table = RoutingTable::new([0; 16]);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 0;
        table
            .handle_update_request(ROUTING_ACCOUNT.clone(), request.clone())
            .unwrap();
        assert_eq!(table.id, request.routing_table_id);
    }

    #[test]
    fn updates_epoch() {
        let mut table = RoutingTable::new(UPDATE_REQUEST_SIMPLE.routing_table_id);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 0;
        request.to_epoch_index = 1;
        table
            .handle_update_request(ROUTING_ACCOUNT.clone(), request.clone())
            .unwrap();
        assert_eq!(table.epoch, 1);

        request.from_epoch_index = 1;
        request.to_epoch_index = 3;
        table
            .handle_update_request(ROUTING_ACCOUNT.clone(), request)
            .unwrap();
        assert_eq!(table.epoch, 3);
    }

    #[test]
    fn errors_if_gap_in_epoch_indecies() {
        let mut table = RoutingTable::new([0; 16]);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 1;
        let result = table.handle_update_request(ROUTING_ACCOUNT.clone(), request);
        assert_eq!(
            result.unwrap_err(),
            "Gap in routing table 21e55f8eabcd4e979ab9bf0ff00a224c. Expected epoch: 0, got from_epoch: 1"
        );
    }

    #[test]
    fn ignores_old_update() {
        let mut table = RoutingTable::new(UPDATE_REQUEST_COMPLEX.routing_table_id);
        table.epoch = 3;
        let mut request = UPDATE_REQUEST_COMPLEX.clone();
        request.from_epoch_index = 0;
        request.to_epoch_index = 1;
        let updated_routes = table
            .handle_update_request(ROUTING_ACCOUNT.clone(), request)
            .unwrap();
        assert_eq!(updated_routes.len(), 0);
    }

    #[test]
    fn ignores_empty_update() {
        let mut table = RoutingTable::new([0; 16]);
        let mut request = UPDATE_REQUEST_SIMPLE.clone();
        request.from_epoch_index = 0;
        request.to_epoch_index = 1;
        let updated_routes = table
            .handle_update_request(ROUTING_ACCOUNT.clone(), request)
            .unwrap();
        assert_eq!(updated_routes.len(), 0);
    }

    #[test]
    fn converts_to_a_simplified_table() {
        let mut table = RoutingTable::new([0; 16]);
        table.add_route(
            TestAccount::new(Uuid::from_slice(&[1; 16]).unwrap(), "example.one"),
            Route {
                prefix: "example.one".to_string(),
                path: Vec::new(),
                props: Vec::new(),
                auth: [0; 32],
            },
        );
        table.add_route(
            TestAccount::new(Uuid::from_slice(&[2; 16]).unwrap(), "example.two"),
            Route {
                prefix: "example.two".to_string(),
                path: Vec::new(),
                props: Vec::new(),
                auth: [0; 32],
            },
        );
        let simplified = table.get_simplified_table();
        assert_eq!(simplified.len(), 2);
        assert_eq!(
            simplified.get("example.one").unwrap().id,
            Uuid::from_slice(&[1; 16]).unwrap()
        );
        assert_eq!(
            simplified.get("example.two").unwrap().id,
            Uuid::from_slice(&[2; 16]).unwrap()
        );
    }
}
