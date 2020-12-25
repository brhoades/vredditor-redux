use std::{iter, path, env};

use rand::{Rng, thread_rng, distributions};

pub(crate) fn random_temp() -> path::PathBuf {
    let mut dir = env::temp_dir();
    let mut rng = thread_rng();
    let chars: String = iter::repeat(())
        .map(|()| rng.sample(distributions::Alphanumeric))
        .take(7)
        .collect();

    dir.push(chars.as_str());
    dir
}
