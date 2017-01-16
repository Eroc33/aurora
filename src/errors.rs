error_chain! {
    foreign_links {
        Io(::std::io::Error);
        Hypeer(::hyper::Error);
    }
}
