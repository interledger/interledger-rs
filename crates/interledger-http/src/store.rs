use futures::Future;

pub struct HttpDetails<'a> {
    pub url: &'a str,
    pub auth_header: &'a str,
}

pub trait HttpStore {
    fn get_account_from_authorization(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = u64, Error = ()>>;
    fn get_http_details_for_account<'a>(
        &self,
        account: u64,
    ) -> Box<Future<Item = HttpDetails<'a>, Error = ()> + 'a>;
}
