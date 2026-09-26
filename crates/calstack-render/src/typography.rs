/// Logical pixel sizes resolved by the platform from desktop typography settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Typography {
    pub caption: f32,
    pub small: f32,
    pub body: f32,
    pub subtitle: f32,
    pub title: f32,
    pub icon: f32,
}
impl Typography {
    pub fn from_base(base: f32) -> Self {
        let px = |ratio: f32| (base * ratio).round().max(1.0);
        Self {
            caption: px(0.833),
            small: px(0.917),
            body: px(1.0),
            subtitle: px(1.083),
            title: px(1.167),
            icon: px(1.167),
        }
    }
    /// Preserve enough room even when a single theme token is independently enlarged.
    pub fn layout_scale(self) -> f32 {
        [
            self.caption / 10.0,
            self.small / 11.0,
            self.body / 12.0,
            self.subtitle / 13.0,
            self.title / 14.0,
            self.icon / 14.0,
        ]
        .into_iter()
        .fold(0.0, f32::max)
    }
}
impl Default for Typography {
    fn default() -> Self {
        Self::from_base(12.0)
    }
}
