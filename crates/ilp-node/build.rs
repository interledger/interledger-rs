fn main() {
    {
        use vergen::{gen, ConstantsFlags};
        let mut flags = ConstantsFlags::empty();
        // there are other flags available as well, this is VERGEN_GIT_SHA_SHORT
        flags.insert(ConstantsFlags::SHA_SHORT);
        gen(flags).expect("Unable to generate the cargo keys! Do you have git installed?");
    }
}
