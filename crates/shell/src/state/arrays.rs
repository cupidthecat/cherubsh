#[derive(Debug, Default, Clone)]
pub struct IndexedArray {
    dense: Vec<Option<String>>,
    sparse: BTreeMap<i64, String>,
    len: usize,
}

impl IndexedArray {
    const MAX_DENSE_GAP: i64 = 1024;
    const MAX_DENSE_LEN: usize = 65_536;

    fn from_values(values: Vec<String>) -> Self {
        let len = values.len();
        Self {
            dense: values.into_iter().map(Some).collect(),
            sparse: BTreeMap::new(),
            len,
        }
    }

    fn from_pairs(values: impl IntoIterator<Item = (i64, String)>) -> Self {
        let mut array = Self::default();
        for (idx, value) in values {
            array.insert(idx, value);
        }
        array
    }

    fn should_store_dense(&self, index: i64) -> bool {
        if index < 0 {
            return false;
        }
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        if index < self.dense.len() {
            return true;
        }
        index < Self::MAX_DENSE_LEN && index <= self.dense.len() + Self::MAX_DENSE_GAP as usize
    }

    fn insert(&mut self, index: i64, value: String) {
        if let Ok(index_usize) = usize::try_from(index) {
            if index_usize == self.dense.len() && index_usize < Self::MAX_DENSE_LEN {
                self.dense.push(Some(value));
                self.len += 1;
                return;
            }
        }
        if self.should_store_dense(index) {
            let index = index as usize;
            if let Some(old) = self.sparse.remove(&(index as i64)) {
                let _ = old;
                self.len = self.len.saturating_sub(1);
            }
            if index >= self.dense.len() {
                self.dense.resize_with(index + 1, || None);
            }
            if self.dense[index].is_none() {
                self.len += 1;
            }
            self.dense[index] = Some(value);
            return;
        }
        if index >= 0 {
            let idx = index as usize;
            if idx < self.dense.len() && self.dense[idx].take().is_some() {
                self.len = self.len.saturating_sub(1);
            }
        }
        if self.sparse.insert(index, value).is_none() {
            self.len += 1;
        }
        self.trim_dense_tail();
    }

    fn get(&self, index: i64) -> Option<&str> {
        if index >= 0 {
            let idx = index as usize;
            if idx < self.dense.len() {
                return self.dense[idx].as_deref();
            }
        }
        self.sparse.get(&index).map(String::as_str)
    }

    fn remove(&mut self, index: i64) -> Option<String> {
        if index >= 0 {
            let idx = index as usize;
            if idx < self.dense.len() {
                let old = self.dense[idx].take();
                if old.is_some() {
                    self.len = self.len.saturating_sub(1);
                    self.trim_dense_tail();
                    return old;
                }
            }
        }
        let old = self.sparse.remove(&index);
        if old.is_some() {
            self.len = self.len.saturating_sub(1);
        }
        old
    }

    fn trim_dense_tail(&mut self) {
        while self.dense.last().is_some_and(Option::is_none) {
            self.dense.pop();
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn max_index(&self) -> Option<i64> {
        let dense_max = self
            .dense
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, value)| value.as_ref().map(|_| idx as i64));
        match (dense_max, self.sparse.keys().next_back().copied()) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn values(&self) -> Vec<String> {
        self.all().into_iter().map(|(_, value)| value).collect()
    }

    fn keys(&self) -> Vec<i64> {
        if self.sparse.is_empty() {
            return self
                .dense
                .iter()
                .enumerate()
                .filter_map(|(idx, value)| value.as_ref().map(|_| idx as i64))
                .collect();
        }
        self.all().into_iter().map(|(idx, _)| idx).collect()
    }

    fn all(&self) -> Vec<(i64, String)> {
        let mut entries = Vec::with_capacity(self.len);
        for (idx, value) in self.dense.iter().enumerate() {
            if let Some(value) = value {
                entries.push((idx as i64, value.clone()));
            }
        }
        entries.extend(self.sparse.iter().map(|(idx, value)| (*idx, value.clone())));
        if !self.sparse.is_empty() {
            entries.sort_by_key(|(idx, _)| *idx);
        }
        entries
    }
}
