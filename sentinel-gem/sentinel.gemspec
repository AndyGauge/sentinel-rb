Gem::Specification.new do |spec|
  spec.name          = "sentinel"
  spec.version       = "0.1.0"
  spec.executables   = ["sentinel"] # The Ruby wrapper in bin/
  spec.bindir        = "bin"
  spec.summary       = "Rust-powered RBS generator for Ruby models."
  spec.description   = "Sentinel scans Ruby files and generates RBS signatures using a high-performance Rust transpiler."
  spec.homepage      = "https://github.com/youruser/sentinel" # Can be a dummy URL
  spec.license       = "MIT"
  spec.authors       = ["Andrew Gauger"]
  spec.email         = ["andygauge@gmail.com"]
  spec.summary       = "Rust-powered RBS generator"
  spec.description   = "A tool to generate RBS signatures from Ruby code using Rust."
  spec.homepage      = "https://github.com/andygauge/sentinel-rs" 
  spec.license       = "MIT"
  # This line is important if you want to ensure you don't accidentally push it
  spec.metadata["allowed_push_host"] = "http://example.com" # Or any dummy string to prevent accidental public push # Include your binaries in the gem files
  spec.files         = Dir["exe/*", "lib/**/*", "bin/*"]
end
