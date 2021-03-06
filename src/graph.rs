use bevy::{math::Vec3Swizzles, prelude::*};
use bevy_kira_audio::{Audio, AudioChannel, AudioSource};
use bevy_rapier2d::prelude::*;
use fxhash::FxHashMap;
use once_cell::sync::Lazy;
use pest::{
    error::{Error, ErrorVariant, LineColLocation},
    iterators::{Pair, Pairs},
    Parser,
};
use std::{iter, time::Duration};

use crate::{
    asset,
    collision::{CollisionGroups, PrevPosition, RocketCollision},
    time::{DelayedEvent, DelayedEventBundle},
    ui::{
        ButtonsEnabled, FunctionDisplayBox, FunctionEntryBox, FunctionStatus, FunctionWhere,
        FunctionX, FunctionY, Textbox, TextboxesEditable,
    },
    z, Field, Game, Owner, Player, PlayerLabel,
};

pub const QUICK_HELP: &str = r"
Examples:
x(t) = v                    x(t) = -2 * t
y(t) = u^2                  y(t) = sin(3 * t)
where u = 2 * t - 4         where
      v = u + t * 0               <nothing>

Operations:
add (+), subtract (-), multiply (*), divide (/),
floor divide (//), modulo (%), exponent (^)

Constants: tau, pi, e

Unary functions (syntax: `sin a`):
sin, cos, tan, asin, acos, atan, sinh, cosh, tanh,
asinh, acosh, atanh, ln, log2, log10, sqrt, cbrt,
abs, sign, floor, ceil, fract

Binary functions (syntax: `min a b`): min, max, atan2

Precedence (highest to lowest):
function call
^
* / // %
+ -
";

#[derive(Parser)]
#[grammar = "function.pest"]
pub struct FunctionParser;

macro_rules! def_str_lookup {
    (
        $(#[$attr:meta])*
        pub enum $enum_name:ident {
            $($var:ident ( $string:tt ) => $func:expr),* $(,)?
        }
        static $set:ident: $set_ty:ty;
        const $arr:ident: $arr_ty:ty;
    ) => {
        $(#[$attr])*
        pub enum $enum_name {
            $($var),*
        }

        static $set: $set_ty = once_cell::sync::Lazy::new(|| [
            $(($string, $enum_name::$var)),*
        ].into_iter().collect());

        const $arr: $arr_ty = [$($func),*];
    };
}

def_str_lookup! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum Call1 {
        Sin("sin") => f64::sin,
        Cos("cos") => f64::cos,
        Tan("tan") => f64::tan,
        Asin("asin") => f64::asin,
        Acos("acos") => f64::acos,
        Atan("atan") => f64::atan,
        Sinh("sinh") => f64::sinh,
        Cosh("cosh") => f64::cosh,
        Tanh("tanh") => f64::tanh,
        Asinh("asinh") => f64::asinh,
        Acosh("acosh") => f64::acosh,
        Atanh("atanh") => f64::atanh,
        Ln("ln") => f64::ln,
        Log2("log2") => f64::log2,
        Log10("log10") => f64::log10,
        Sqrt("sqrt") => f64::sqrt,
        Cbrt("cbrt") => f64::cbrt,
        Abs("abs") => f64::abs,
        Sign("sign") => f64::signum,
        Floor("floor") => f64::floor,
        Ceil("ceil") => f64::ceil,
        Fract("fract") => f64::fract,
    }

    static CALL_1_FN_MAP: Lazy<FxHashMap<&str, Call1>>;

    const CALL_1_FNS: [fn(f64) -> f64; 22];
}

impl Call1 {
    fn call(self, t: f64) -> f64 {
        CALL_1_FNS[self as usize](t)
    }
}

def_str_lookup! {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum Call2 {
        Min("min") => f64::min,
        Max("max") => f64::max,
        Atan2("atan2") => f64::atan2,
    }

    static CALL_2_FN_MAP: Lazy<FxHashMap<&str, Call2>>;

    const CALL_2_FNS: [fn(f64, f64) -> f64; 3];
}

impl Call2 {
    fn call(self, t1: f64, t2: f64) -> f64 {
        CALL_2_FNS[self as usize](t1, t2)
    }
}

static CONSTS: Lazy<FxHashMap<&str, f64>> = Lazy::new(|| {
    [("tau", std::f64::consts::TAU), ("pi", std::f64::consts::PI), ("e", std::f64::consts::E)]
        .into_iter()
        .collect()
});

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OpType {
    Normal,
    Inverse,
    Third,
    Fourth,
}

#[derive(Clone, Debug)]
pub enum Function {
    /// If the option is None, the variable is `t`
    Var(Option<usize>),
    Const(f64),
    Add(Vec<(Function, OpType)>),
    Mul(Vec<(Function, OpType)>),
    Exp(Vec<Function>),
    Neg(Box<Function>),
    Call1(Call1, Box<Function>),
    Call2(Call2, Box<[Function; 2]>),
}

/// Labels a rocket
#[derive(Component)]
pub struct Rocket;

const ROCKET_TIME: f32 = 5.0;

/// The offset of a rocket from the parametric equation it follows
#[derive(Component)]
pub struct Offset(Vec2);

impl Function {
    fn from_multi_op_sequence(
        pair: Pair<Rule>,
        variant: impl Fn(Vec<(Function, OpType)>) -> Self,
        signs: &[(&str, OpType)],
        var_map: &VarIndexMap,
    ) -> Result<Self, Error<Rule>> {
        let mut inner = pair.into_inner();
        let first = inner.next().unwrap();
        if inner.peek().is_some() {
            let pair_vec = inner.collect::<Vec<_>>();
            Ok(variant(
                iter::once(Self::from_pair(first, var_map).map(|f| (f, OpType::Normal)))
                    .chain(pair_vec.chunks(2).map(|pairs| {
                        let op_sign = &pairs[0];
                        let expr = pairs[1].clone();
                        Self::from_pair(expr, var_map).map(|f| {
                            (
                                f,
                                signs
                                    .iter()
                                    .find_map(|(sign, op)| (*sign == op_sign.as_str()).then(|| *op))
                                    .unwrap(),
                            )
                        })
                    }))
                    .collect::<Result<_, _>>()?,
            ))
        } else {
            Self::from_pair(first, var_map)
        }
    }

    fn from_op_sequence(
        pair: Pair<Rule>,
        variant: impl Fn(Vec<Function>) -> Self,
        var_map: &VarIndexMap,
    ) -> Result<Self, Error<Rule>> {
        let mut inner = pair.into_inner();
        let first = inner.next().unwrap();
        if inner.peek().is_some() {
            Ok(variant(
                iter::once(first)
                    .chain(inner)
                    .map(|p| Self::from_pair(p, var_map))
                    .collect::<Result<_, _>>()?,
            ))
        } else {
            Self::from_pair(first, var_map)
        }
    }

    fn from_pair(pair: Pair<Rule>, var_map: &VarIndexMap) -> Result<Self, Error<Rule>> {
        match pair.as_rule() {
            Rule::expr => Self::from_pair(pair.into_inner().next().unwrap(), var_map),
            Rule::add => Self::from_multi_op_sequence(
                pair,
                Self::Add,
                &[("+", OpType::Normal), ("-", OpType::Inverse)],
                var_map,
            ),
            Rule::mul => Self::from_multi_op_sequence(
                pair,
                Self::Mul,
                &[
                    ("*", OpType::Normal),
                    ("/", OpType::Inverse),
                    ("//", OpType::Third),
                    ("%", OpType::Fourth),
                ],
                var_map,
            ),
            Rule::neg => {
                let negate = pair.as_str().starts_with('-');
                let expr = Self::from_pair(pair.into_inner().next().unwrap(), var_map)?;
                if negate {
                    Ok(Self::Neg(Box::new(expr)))
                } else {
                    Ok(expr)
                }
            }
            Rule::exp => Self::from_op_sequence(pair, Self::Exp, var_map),
            Rule::call_1 => {
                let mut pairs = pair.into_inner();
                let func = pairs.next().unwrap();
                let expr = pairs.next().unwrap();
                if let Some(call) = CALL_1_FN_MAP.get(func.as_str()) {
                    Ok(Self::Call1(*call, Box::new(Self::from_pair(expr, var_map)?)))
                } else {
                    Err(Error::new_from_span(
                        ErrorVariant::CustomError {
                            message: format!("unknown unary function: {}", func.as_str()),
                        },
                        func.as_span(),
                    ))
                }
            }
            Rule::call_2 => {
                let mut pairs = pair.into_inner();
                let func = pairs.next().unwrap();
                let expr1 = pairs.next().unwrap();
                let expr2 = pairs.next().unwrap();
                if let Some(call) = CALL_2_FN_MAP.get(func.as_str()) {
                    Ok(Self::Call2(
                        *call,
                        Box::new([
                            Self::from_pair(expr1, var_map)?,
                            Self::from_pair(expr2, var_map)?,
                        ]),
                    ))
                } else {
                    Err(Error::new_from_span(
                        ErrorVariant::CustomError {
                            message: format!("unknown binary function: {}", func.as_str()),
                        },
                        func.as_span(),
                    ))
                }
            }
            Rule::primary => Self::from_pair(pair.into_inner().next().unwrap(), var_map),
            Rule::primitive => Self::from_pair(pair.into_inner().next().unwrap(), var_map),
            Rule::var => {
                if let Some(constant) = CONSTS.get(pair.as_str()) {
                    Ok(Self::Const(*constant))
                } else if let Some(index) = var_map.get(pair.as_str()) {
                    Ok(Self::Var(*index))
                } else {
                    Err(Error::new_from_span(
                        ErrorVariant::CustomError {
                            message: format!("unknown variable: {}", pair.as_str()),
                        },
                        pair.as_span(),
                    ))
                }
            }
            Rule::constant => Ok(Self::Const(str::parse(pair.as_str()).unwrap())),

            _ => unreachable!(),
        }
    }

    fn eval(&self, t: f64, assigns: &[Function]) -> f64 {
        match self {
            Self::Var(index) => index.map(|i| assigns[i].eval(t, assigns)).unwrap_or(t),
            Self::Const(c) => *c,
            Self::Add(fs) => fs.iter().fold(0.0, |acc, (f, op)| match *op {
                OpType::Normal => acc + f.eval(t, assigns),
                OpType::Inverse => acc - f.eval(t, assigns),
                _ => unreachable!(),
            }),
            Self::Mul(fs) => fs.iter().fold(1.0, |acc, (f, op)| match *op {
                OpType::Normal => acc * f.eval(t, assigns),
                OpType::Inverse => acc / f.eval(t, assigns),
                OpType::Third => acc.div_euclid(f.eval(t, assigns)),
                OpType::Fourth => acc.rem_euclid(f.eval(t, assigns)),
            }),
            Self::Exp(fs) => fs.iter().rev().fold(1.0, |acc, f| f.eval(t, assigns).powf(acc)),
            Self::Neg(f) => -f.eval(t, assigns),
            Self::Call1(call, f) => call.call(f.eval(t, assigns)),
            Self::Call2(call, fs) => call.call(fs[0].eval(t, assigns), fs[1].eval(t, assigns)),
        }
    }
}

/// Maps variable indexes to functions
type AssignVec = Vec<Function>;

/// Maps variable names to indexes
type VarIndexMap = FxHashMap<String, Option<usize>>;

trait Assigns: Sized {
    fn from_pairs(pairs: Pairs<Rule>) -> Result<(Self, VarIndexMap), Error<Rule>>;
}

impl Assigns for AssignVec {
    fn from_pairs(pairs: Pairs<Rule>) -> Result<(Self, VarIndexMap), Error<Rule>> {
        let mut var_map = [("t".to_owned(), None)].into_iter().collect::<FxHashMap<_, _>>();

        let assign_vec = pairs
            .filter(|pair| pair.as_rule() != Rule::EOI)
            .enumerate()
            .map(|(i, pair)| {
                let mut pairs = pair.into_inner();
                let var = pairs.next().unwrap();
                let var = if var_map.contains_key(var.as_str()) {
                    return Err(Error::new_from_span(
                        ErrorVariant::CustomError {
                            message: format!("'{}' is already defined", var.as_str()),
                        },
                        var.as_span(),
                    ));
                } else if CONSTS.contains_key(var.as_str()) {
                    return Err(Error::new_from_span(
                        ErrorVariant::CustomError {
                            message: format!("cannot assign to constant '{}'", var.as_str()),
                        },
                        var.as_span(),
                    ));
                } else {
                    var.as_str().to_owned()
                };

                var_map.insert(var, Some(i));
                let expr = Function::from_pair(pairs.next().unwrap(), &var_map)?;
                Ok(expr)
            })
            .collect::<Result<_, _>>()?;

        Ok((assign_vec, var_map))
    }
}

#[derive(Clone, Debug, Component)]
pub struct Parametric {
    pub x: Function,
    pub y: Function,
    pub assigns: AssignVec,
    pub source_x: Option<String>,
    pub source_y: Option<String>,
    pub source_assigns: Option<String>,
}

impl Parametric {
    pub fn new(
        x: Function,
        y: Function,
        assigns: AssignVec,
        source_x: String,
        source_y: String,
        source_assigns: String,
    ) -> Self {
        Self {
            x,
            y,
            assigns,
            source_x: Some(source_x),
            source_y: Some(source_y),
            source_assigns: Some(source_assigns),
        }
    }

    fn eval(&self, t: f64) -> Vec2 {
        Vec2::new(self.x.eval(t, &self.assigns) as f32, self.y.eval(t, &self.assigns) as f32)
    }
}

#[derive(Clone, Debug)]
/// Event that says that some player should queue a rocket to be fired from their position
pub struct SendFunctions {
    pub player_index: u32,
}

struct ParseError {
    error: Error<Rule>,
    label: String,
    include_line: bool,
}

impl ParseError {
    fn new(error: Error<Rule>, label: String, include_line: bool) -> Self {
        Self { error, label, include_line }
    }
}

fn set_status_text(text: &mut Text, error: Option<ParseError>) {
    if let Some(error) = error {
        let message_end = match error.error.variant {
            ErrorVariant::CustomError { message } => message,
            ErrorVariant::ParsingError { .. } => "syntax".into(),
        };
        let (line, column) = match error.error.line_col {
            LineColLocation::Pos((l, c)) | LineColLocation::Span((l, c), _) => (l, c),
        };
        let line_message =
            if error.include_line { format!("line {} ", line) } else { String::new() };
        let message =
            format!("Error in {} ({}col {}): {}\n", error.label, line_message, column, message_end);
        text.sections[0].value = message;
        text.sections[0].style.color = Color::MAROON;
    } else {
        text.sections[0].value = "Successfully entered functions\n".into();
        text.sections[0].style.color = Color::DARK_GREEN;
    }
}

pub fn send_functions(
    function_x: Query<(&Owner, &Textbox), (With<FunctionX>, With<FunctionEntryBox>)>,
    function_y: Query<(&Owner, &Textbox), (With<FunctionY>, With<FunctionEntryBox>)>,
    assigns: Query<(&Owner, &Textbox), (With<FunctionWhere>, With<FunctionEntryBox>)>,
    mut players: ResMut<Vec<Player>>,
    mut status: Query<&mut Text, With<FunctionStatus>>,
    mut fire_events: EventReader<SendFunctions>,
    mut commands: Commands,
    mut textboxes_editable: ResMut<TextboxesEditable>,
    mut buttons_enabled: ResMut<ButtonsEnabled>,
    field: Query<Entity, With<Field>>,
) {
    'main: for event in fire_events.iter() {
        let mut status_text = status.single_mut();
        let player = event.player_index;

        let fx_str = function_x
            .iter()
            .find_map(|(owner, textbox)| (owner.0 == player).then(|| &textbox.text))
            .unwrap();
        let fy_str = function_y
            .iter()
            .find_map(|(owner, textbox)| (owner.0 == player).then(|| &textbox.text))
            .unwrap();
        let where_str = assigns
            .iter()
            .find_map(|(owner, textbox)| (owner.0 == player).then(|| &textbox.text))
            .unwrap();

        let (assigns, var_map) = match FunctionParser::parse(Rule::assigns, where_str) {
            Ok(mut pairs) => {
                let assign_pairs = pairs.next().unwrap().into_inner();
                match AssignVec::from_pairs(assign_pairs) {
                    Ok(assigns) => assigns,
                    Err(error) => {
                        set_status_text(
                            &mut *status_text,
                            Some(ParseError::new(error, "'where'".into(), true)),
                        );
                        continue 'main;
                    }
                }
            }

            Err(error) => {
                set_status_text(
                    &mut *status_text,
                    Some(ParseError::new(error, "'where'".into(), true)),
                );
                continue 'main;
            }
        };

        let mut funcs = Vec::with_capacity(2);

        for (axis, func) in [("x", fx_str), ("y", fy_str)] {
            match FunctionParser::parse(Rule::func, func) {
                Ok(mut pairs) => {
                    let func = pairs.next().unwrap();
                    let expr = func.into_inner().next().unwrap();
                    match Function::from_pair(expr, &var_map) {
                        Ok(f) => funcs.push(f),
                        Err(error) => {
                            set_status_text(
                                &mut *status_text,
                                Some(ParseError::new(error, format!("{}(t)", axis), false)),
                            );
                            continue 'main;
                        }
                    }
                }

                Err(error) => {
                    set_status_text(
                        &mut *status_text,
                        Some(ParseError::new(error, format!("{}(t)", axis), false)),
                    );
                    continue 'main;
                }
            }
        }

        let fy = funcs.pop().unwrap();
        let fx = funcs.pop().unwrap();

        let parametric =
            Parametric::new(fx, fy, assigns, fx_str.clone(), fy_str.clone(), where_str.clone());

        set_status_text(&mut *status_text, None);

        players[player as usize].parametric = Some(parametric);

        commands.entity(field.single()).with_children(|node| {
            node.spawn_bundle(DelayedEventBundle::new(1.0, DelayedEvent::AdvanceTurn));
        });
        textboxes_editable.0 = false;
        buttons_enabled.0 = false;
    }
}

/// t reached 1, so the rocket's time is up. This is an event.
pub struct RocketTimeUp {
    pub rocket: Entity,
}

/// Labels a graph constructed by a rocket.
#[derive(Component)]
pub struct Graph {
    color: Color,
    rocket: Entity,
}

/// Audio channel for a rocket.
#[derive(Component)]
pub struct RocketChannel(pub AudioChannel);

const GRAPH_COLORS: [Color; 4] = [Color::RED, Color::CYAN, Color::YELLOW, Color::GREEN];

pub fn fire_rockets(
    mut commands: Commands,
    mut textboxes_fx: Query<(&Owner, &mut Textbox), (With<FunctionDisplayBox>, With<FunctionX>)>,
    mut textboxes_fy: Query<
        (&Owner, &mut Textbox),
        (With<FunctionDisplayBox>, With<FunctionY>, Without<FunctionX>),
    >,
    mut textboxes_where: Query<
        (&Owner, &mut Textbox),
        (With<FunctionDisplayBox>, With<FunctionWhere>, Without<FunctionX>, Without<FunctionY>),
    >,
    mut players: ResMut<Vec<Player>>,
    player_comps: Query<(&Owner, &GlobalTransform), With<PlayerLabel>>,
    images: Res<Assets<Image>>,
    field: Query<Entity, With<Field>>,
    audio: Res<Audio>,
    sounds: Res<Assets<AudioSource>>,
) {
    for (owner, mut textbox) in textboxes_fx.iter_mut() {
        if let Some(player) = players.get_mut(owner.0 as usize) {
            let parametric = player.parametric.as_mut().unwrap();
            textbox.text = parametric.source_x.take().unwrap();
        }
    }
    for (owner, mut textbox) in textboxes_fy.iter_mut() {
        if let Some(player) = players.get_mut(owner.0 as usize) {
            let parametric = player.parametric.as_mut().unwrap();
            textbox.text = parametric.source_y.take().unwrap();
        }
    }
    for (owner, mut textbox) in textboxes_where.iter_mut() {
        if let Some(player) = players.get_mut(owner.0 as usize) {
            let parametric = player.parametric.as_mut().unwrap();
            textbox.text = parametric.source_assigns.take().unwrap();
        }
    }

    let fire_channel = AudioChannel::new("Fire".into());
    audio.play_in_channel(sounds.get_handle(asset::Fire), &fire_channel);
    audio.set_volume_in_channel(2.0, &fire_channel);

    commands.entity(field.single()).with_children(|node| {
        for (owner, transform) in player_comps.iter() {
            let player = owner.0;
            let parametric = players[player as usize].parametric.take().unwrap();
            let start = parametric.eval(0.0);
            let scale = 0.3;

            let channel = AudioChannel::new(owner.0.to_string());
            audio.play_looped_in_channel(sounds.get_handle(asset::RocketMove(owner.0)), &channel);

            let rocket = node
                .spawn_bundle(SpriteBundle {
                    sprite: Sprite { custom_size: Some(Vec2::new(2.8, 1.4)), ..Default::default() },
                    texture: images.get_handle(asset::Rocket(owner.0)),
                    transform: Transform::from(*transform).with_scale([scale; 3].into()),
                    ..Default::default()
                })
                .insert(parametric)
                .insert(Offset(transform.translation.xy() - start))
                .insert(Rocket)
                .insert(Timer::new(Duration::from_secs_f32(ROCKET_TIME), false))
                .insert(Owner(player))
                .insert(PrevPosition(transform.translation.xy()))
                .insert(RocketChannel(channel))
                .insert_bundle(RigidBodyBundle {
                    body_type: RigidBodyType::KinematicPositionBased.into(),
                    position: transform.translation.xy().extend(0.0).into(),
                    // kinematic-static CCD doesn't work
                    ..Default::default()
                })
                .with_children(|body| {
                    body.spawn_bundle(ColliderBundle {
                        shape: ColliderShape::ball(scale / 2.0).into(),
                        collider_type: ColliderType::Solid.into(),
                        position: Vec2::ZERO.into(),
                        flags: ColliderFlags {
                            collision_groups: InteractionGroups::new(
                                CollisionGroups::ROCKET.bits(),
                                CollisionGroups::ROCKET_CAST.bits(),
                            ),
                            active_collision_types: ActiveCollisionTypes::KINEMATIC_KINEMATIC
                                | ActiveCollisionTypes::KINEMATIC_STATIC,
                            ..Default::default()
                        }
                        .into(),
                        ..Default::default()
                    });
                })
                .id();

            node.spawn()
                .insert(Transform::identity())
                .insert(GlobalTransform::identity())
                .insert(*owner)
                .insert(Graph { color: GRAPH_COLORS[owner.0 as usize], rocket });
        }
    });
}

pub fn move_rockets(
    mut rockets: Query<
        (
            &mut Transform,
            &mut RigidBodyPositionComponent,
            &Offset,
            &Parametric,
            &mut Timer,
            Entity,
            &RigidBodyCollidersComponent,
            &RocketChannel,
        ),
        With<Rocket>,
    >,
    mut commands: Commands,
    time: Res<Time>,
    mut buttons_enabled: ResMut<ButtonsEnabled>,
    mut time_up_events: EventWriter<RocketTimeUp>,
    audio: Res<Audio>,
    game: Res<Game>,
) {
    let mut rockets_exist = false;
    for (
        mut transform,
        mut body_position,
        offset,
        parametric,
        mut timer,
        entity,
        colliders,
        channel,
    ) in rockets.iter_mut()
    {
        // The colliders are missing for 1 frame, so skip that frame
        if colliders.0 .0.is_empty() {
            return;
        }

        rockets_exist = true;
        timer.tick(time.delta());
        if timer.finished() {
            commands.entity(entity).despawn_recursive();
            time_up_events.send(RocketTimeUp { rocket: entity });
        }

        let next_pos = parametric.eval(timer.percent() as f64) + offset.0;
        let curr_pos = transform.translation.xy();
        if next_pos - curr_pos != Vec2::ZERO {
            transform.rotation =
                Quat::from_rotation_arc_2d(Vec2::X, (next_pos - curr_pos).normalize());
        }
        transform.translation = next_pos.extend(z::ROCKET);
        body_position.0.next_position =
            Isometry::new(next_pos.into(), transform.rotation.to_axis_angle().1);

        // Sound modulation
        const MAX_VOLUME_SPEED: f32 = 15.0 / ROCKET_TIME;
        const MAX_VOLUME: f32 = 3.0;
        let scale = game.scale;
        let speed = ((next_pos - curr_pos).length() / time.delta_seconds()).min(MAX_VOLUME_SPEED);
        audio.set_panning_in_channel((next_pos.x - -scale) / (2.0 * scale), &channel.0);
        audio.set_volume_in_channel(speed / MAX_VOLUME_SPEED * MAX_VOLUME, &channel.0);
    }

    if !rockets_exist {
        buttons_enabled.0 = true;
    }
}

pub fn stop_rocket_sounds(
    mut collisions: EventReader<RocketCollision>,
    mut time_ups: EventReader<RocketTimeUp>,
    audio: Res<Audio>,
    channels: Query<&RocketChannel>,
) {
    let mut num_stopped_rockets = 0;

    for collision in collisions.iter() {
        for entity in [collision.rocket, collision.other] {
            if let Ok(channel) = channels.get(entity) {
                audio.stop_channel(&channel.0);
                num_stopped_rockets += 1;
            }
        }
    }

    for time_up in time_ups.iter() {
        if let Ok(channel) = channels.get(time_up.rocket) {
            audio.stop_channel(&channel.0);
        }
    }

    if num_stopped_rockets > 0 && num_stopped_rockets == channels.iter().len() {
        audio.stop_channel(&AudioChannel::new("Fire".into()));
    }
}

pub fn graph_functions(
    graphs: Query<(Entity, &Graph)>,
    rockets: Query<(&PrevPosition, &Transform), With<Rocket>>,
    mut commands: Commands,
) {
    const GRAPH_THICKNESS: f32 = 0.03;

    for (entity, graph) in graphs.iter() {
        let (prev_pos, curr_transform) =
            if let Ok(r) = rockets.get(graph.rocket) { r } else { continue };
        let prev_pos = prev_pos.0;
        let curr_pos = curr_transform.translation.xy();
        if prev_pos == curr_pos {
            continue;
        }

        let line_pos = ((prev_pos + curr_pos) / 2.0).extend(z::GRAPH);
        let line_rot = Quat::from_rotation_arc_2d(Vec2::X, (curr_pos - prev_pos).normalize());
        let line_size = Vec2::new((curr_pos - prev_pos).length(), GRAPH_THICKNESS);
        commands.entity(entity).with_children(|node| {
            node.spawn_bundle(SpriteBundle {
                sprite: Sprite {
                    color: graph.color,
                    custom_size: Some(line_size),
                    ..Default::default()
                },
                transform: Transform::from_rotation(line_rot).with_translation(line_pos),
                ..Default::default()
            });
        });
    }
}
