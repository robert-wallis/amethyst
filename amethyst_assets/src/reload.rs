//! Defines the `Reload` trait.

use std::sync::Arc;
use std::time::Instant;

use amethyst_core::ECSBundle;
use specs::{DispatcherBuilder, FetchMut, System, World};

use {Asset, BoxedErr, Format, FormatValue, Loader, Source};

/// This bundle activates hot reload for the `Loader`,
/// adds a `HotReloadStrategy` and the `HotReloadSystem`.
///
/// **NOTE:** Add this only after all the asset processing systems.
#[derive(Default)]
pub struct HotReloadBundle {
    strategy: HotReloadStrategy,
}

impl HotReloadBundle {
    /// Creates a new bundle.
    pub fn new(strategy: HotReloadStrategy) -> Self {
        HotReloadBundle { strategy }
    }
}

impl<'a, 'b> ECSBundle<'a, 'b> for HotReloadBundle {
    fn build(
        self,
        world: &mut World,
        dispatcher: DispatcherBuilder<'a, 'b>,
    ) -> Result<DispatcherBuilder<'a, 'b>, BoxedErr> {
        world.write_resource::<Loader>().set_hot_reload(true);
        world.add_resource(self.strategy);

        Ok(dispatcher.add(HotReloadSystem, "hot_reload", &[]))
    }
}

/// An ECS resource which allows to configure hot reloading.
///
/// ## Examples
///
/// ```
/// # extern crate amethyst_assets;
/// # extern crate specs;
/// #
/// # use amethyst_assets::HotReloadStrategy;
/// # use specs::World;
/// #
/// # fn main() {
/// let mut world = World::new();
/// // Assets will be reloaded every two seconds (in case they changed)
/// world.add_resource(HotReloadStrategy::every(2));
/// # }
/// ```
pub struct HotReloadStrategy {
    inner: HotReloadStrategyInner,
}

impl HotReloadStrategy {
    /// Causes hot reloads every `n` seconds.
    pub fn every(n: u8) -> Self {
        HotReloadStrategy {
            inner: HotReloadStrategyInner::Every {
                interval: n,
                last: Instant::now(),
                do_reload: false,
            },
        }
    }

    /// This allows to use `trigger` for hot reloading.
    pub fn when_triggered() -> Self {
        HotReloadStrategy {
            inner: HotReloadStrategyInner::Trigger { triggered: false },
        }
    }

    /// Never do any hot-reloading.
    pub fn never() -> Self {
        HotReloadStrategy {
            inner: HotReloadStrategyInner::Never,
        }
    }

    /// The frame after calling this, all changed assets will be reloaded.
    /// Doesn't do anything if the strategy wasn't created with `when_triggered`.
    pub fn trigger(&mut self) {
        if let HotReloadStrategyInner::Trigger { ref mut triggered } = self.inner {
            *triggered = true;
        }
    }

    /// Crate-internal method to check if reload is necessary.
    /// `reload_counter` is a per-storage value which is only used
    /// for and by this method.
    pub(crate) fn needs_reload(&self) -> bool {
        match self.inner {
            HotReloadStrategyInner::Every { do_reload, .. } => do_reload,
            HotReloadStrategyInner::Trigger { triggered } => triggered,
            HotReloadStrategyInner::Never => false,
        }
    }
}

impl Default for HotReloadStrategy {
    fn default() -> Self {
        HotReloadStrategy::every(1)
    }
}

enum HotReloadStrategyInner {
    Every {
        interval: u8,
        last: Instant,
        do_reload: bool,
    },
    Trigger { triggered: bool },
    Never,
}

/// System for updating `HotReloadStrategy`.
/// **NOTE:** You have to add this after all asset processing systems.
pub struct HotReloadSystem;

impl<'a> System<'a> for HotReloadSystem {
    type SystemData = FetchMut<'a, HotReloadStrategy>;

    fn run(&mut self, mut strategy: Self::SystemData) {
        match strategy.inner {
            HotReloadStrategyInner::Trigger { ref mut triggered } => {
                *triggered = false;
            }
            HotReloadStrategyInner::Every {
                interval,
                ref mut last,
                ref mut do_reload,
            } => if last.elapsed().as_secs() > interval as u64 {
                *do_reload = true;
                *last = Instant::now();
            } else {
                *do_reload = false
            },
            HotReloadStrategyInner::Never => {}
        }
    }
}

/// The `Reload` trait provides a method which checks if an asset needs to be reloaded.
pub trait Reload<A: Asset>: ReloadClone<A> + Send + Sync + 'static {
    /// Checks if a reload is necessary.
    fn needs_reload(&self) -> bool;
    /// Returns the asset name.
    fn name(&self) -> String;
    /// Returns the format name.
    fn format(&self) -> &'static str;
    /// Reloads the asset.
    fn reload(self: Box<Self>) -> Result<FormatValue<A>, BoxedErr>;
}

pub trait ReloadClone<A> {
    fn cloned(&self) -> Box<Reload<A>>;
}

impl<A, T> ReloadClone<A> for T
where
    A: Asset,
    T: Clone + Reload<A>,
{
    fn cloned(&self) -> Box<Reload<A>> {
        Box::new(self.clone())
    }
}

impl<A: Asset> Clone for Box<Reload<A>> {
    fn clone(&self) -> Self {
        self.cloned()
    }
}

/// An implementation of `Reload` which just stores the modification time
/// and the path of the file.
pub struct SingleFile<A: Asset, F: Format<A>> {
    format: F,
    modified: u64,
    options: F::Options,
    path: String,
    source: Arc<Source>,
}

impl<A: Asset, F: Format<A>> SingleFile<A, F> {
    /// Creates a new `SingleFile` reload object.
    pub fn new(
        format: F,
        modified: u64,
        options: F::Options,
        path: String,
        source: Arc<Source>,
    ) -> Self {
        SingleFile {
            format,
            modified,
            options,
            path,
            source,
        }
    }
}

impl<A, F> Clone for SingleFile<A, F>
where
    A: Asset,
    F: Clone + Format<A>,
    F::Options: Clone,
{
    fn clone(&self) -> Self {
        SingleFile {
            format: self.format.clone(),
            modified: self.modified,
            options: self.options.clone(),
            path: self.path.clone(),
            source: self.source.clone(),
        }
    }
}

impl<A, F> Reload<A> for SingleFile<A, F>
where
    A: Asset,
    F: Clone + Format<A> + Sync,
    <F as Format<A>>::Options: Clone + Sync,
{
    fn needs_reload(&self) -> bool {
        self.modified != 0 && (self.source.modified(&self.path).unwrap_or(0) > self.modified)
    }

    fn reload(self: Box<Self>) -> Result<FormatValue<A>, BoxedErr> {
        let this: SingleFile<_, _> = *self;
        let SingleFile {
            format,
            path,
            source,
            options,
            ..
        } = this;

        format.import(path, source, options, true)
    }

    fn name(&self) -> String {
        self.path.clone()
    }

    fn format(&self) -> &'static str {
        F::NAME
    }
}
