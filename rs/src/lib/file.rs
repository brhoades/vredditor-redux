use std::{env, iter, path};

use rand::{distributions, thread_rng, Rng};

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
