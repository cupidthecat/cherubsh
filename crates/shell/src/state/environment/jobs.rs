macro_rules! environment_jobs {
    () => {
        fn jobs_table(&self) -> Option<&JobTable> {
            Some(&self.jobs)
        }
        fn jobs_table_mut(&mut self) -> Option<&mut JobTable> {
            Some(&mut self.jobs)
        }
    };
}
