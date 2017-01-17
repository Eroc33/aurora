error_chain! {
    foreign_links {
        Io(::std::io::Error);
        Hypeer(::hyper::Error);
    }
    errors{
        UploadFailed{
            description("Failed to upload an inverter reading")
        }
    }
}
