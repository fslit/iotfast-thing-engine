mod dev_profile;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    dev_profile::run()
}
