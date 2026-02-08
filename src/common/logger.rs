// Optimized logging - 90% reduction, only critical logs
// Direct println! for critical logs to avoid channel overhead

#[derive(Clone)]
pub struct Logger {
    prefix: String,
}

impl Logger {
    pub fn new(prefix: String) -> Self {
        Logger { prefix }
    }

    // Disabled for performance - 90% reduction
    // Only use critical() or error() for important logs
    #[inline(always)]
    pub fn log(&self, _message: String) {
        // No-op for performance
    }

    // Critical errors only - direct println to avoid channel overhead
    #[inline(always)]
    pub fn error(&self, message: String) {
        println!("{} [ERROR] {}", self.prefix, message);
    }

    // Debug disabled for performance
    #[inline(always)]
    pub fn debug(&self, _message: String) {
        // No-op
    }
    
    // Success disabled - use critical() for important successes
    #[inline(always)]
    pub fn success(&self, _message: String) {
        // No-op
    }
    
    // Critical logs only - direct println for minimal overhead
    #[inline(always)]
    pub fn log_critical(&self, message: String) {
        println!("{} {}", self.prefix, message);
    }
    
    // New method for critical messages that should always be logged
    #[inline(always)]
    pub fn critical(&self, message: String) {
        println!("{} {}", self.prefix, message);
    }
}
