use bevy::asset::HandleId;
use bevy::prelude::*;

macro_rules! assets {
    (
        $(#[$attr:meta])*
        pub enum $enum_name:ident {
            $($var:ident $( ( $(* $tname:ident : $ttype:ty),* ) )? => ($($exprs:expr),*)),* $(,)?
        }
    ) => {
        $(#[$attr])*
        pub enum $enum_name {
            $($var $( ($($ttype),*) )?),*
        }

        impl std::fmt::Display for $enum_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$var $( ($($tname),*) )? => write!(f, $($exprs),*)),*
                }
            }
        }
    };
}

assets! {
    /// Identifiers for all assets of the game
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Asset {
        Title => ("title.png"),
        Ball => ("ball.png"),
        Boom => ("boom.png"),
        Mine => ("mine.png"),
        Player(*i: u32) => ("player{}.png", *i + 1),
        Rocket(*i: u32) => ("rocket{}.png", *i + 1),
        Font => ("NotoMono-Regular.ttf"),
        BallPickup => ("ball_pickup.ogg"),
        PlayerBallPickup => ("player_ball_pickup.ogg"),
        Explosion => ("explosion.ogg"),
        //RocketMove => ("rocket_move.ogg"),
        RocketMove(*i: u32) => ("rocket_move{}.ogg", *i + 1),
        Fire => ("fire.ogg"),
    }
}

impl From<Asset> for HandleId {
    fn from(asset: Asset) -> Self {
        asset.to_string().into()
    }
}

pub fn load_assets(asset_server: Res<AssetServer>, mut used_assets: ResMut<Vec<HandleUntyped>>) {
    // load_folder doesn't work in wasm
    for asset_path in [
        "ball.png",
        "boom.png",
        "mine.png",
        "title.png",
        "NotoMono-Regular.ttf",
        "ball_pickup.ogg",
        "player_ball_pickup.ogg",
        "explosion.ogg",
        "fire.ogg",
        //"rocket_move.ogg",
    ]
    .into_iter()
    .map(|s| s.to_owned())
    .chain((1..=4).map(|i| format!("player{}.png", i)))
    .chain((1..=4).map(|i| format!("rocket{}.png", i)))
    .chain((1..=4).map(|i| format!("rocket_move{}.ogg", i)))
    {
        used_assets.push(asset_server.load_untyped(&asset_path));
    }
    //*used_assets = asset_server.load_folder("./.").expect("Could not load assets");
}

pub use Asset::*;
