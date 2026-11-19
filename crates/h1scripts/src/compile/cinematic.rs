use super::*;
use crate::cinematics::{Field, MARKER};

impl Compiler<'_> {
    fn cine_global(&mut self, name: &str, ty: u16, value: i32) -> Result<()> {
        let index = self.out.globals.len();
        if index >= hs::MAX_DEFINITIONS
            || self
                .t
                .globals
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(format!("no scenario global available for {name}")));
        }
        let root = self.number(ty, value);
        self.out.globals.push(definition(name, ty, 0, root, index)?);
        Ok(())
    }

    pub(super) fn cinematic_marker(&mut self) -> Result<()> {
        if let Some(base) = self.cinematic_base {
            self.cine_global(MARKER, h2t::LONG, i32::from(base))?;
        }
        Ok(())
    }

    fn cine_var(&mut self, field: Field) -> Datum {
        let string = self.intern(field.name().as_bytes());
        self.push(New {
            opcode: field.ty(),
            ty: field.ty(),
            flags: GLOBAL,
            string,
            data: u32::from(self.cinematic_base.unwrap()) + field as u32,
        })
    }

    fn cine_set(&mut self, field: Field, value: Datum) -> Result<Datum> {
        let var = self.cine_var(field);
        self.control_call("set", h2t::VOID, &[var, value])
    }

    fn cine_const(&mut self, field: Field, value: i32) -> Result<Datum> {
        let value = self.number(field.ty(), value);
        self.cine_set(field, value)
    }

    pub(super) fn cinematic_call(
        &mut self,
        name: &str,
        args: &[Datum],
        expected: u16,
    ) -> Result<Option<Datum>> {
        use Field::*;
        let fields: &[Field] = match name {
            "cinematic_screen_effect_start" => &[Clear],
            "cinematic_screen_effect_stop" => &[],
            "cinematic_screen_effect_set_convolution" => {
                &[Passes, Kernel, RadiusFrom, RadiusTo, BlurSeconds]
            }
            "cinematic_screen_effect_set_filter" => &[
                LightFrom,
                LightTo,
                DesatFrom,
                DesatTo,
                FilterFlag,
                FilterSeconds,
            ],
            "cinematic_screen_effect_set_filter_desaturation_tint" => &[Red, Green, Blue],
            "cinematic_screen_effect_set_video" => &[VideoMode, Noise],
            "rasterizer_model_ambient_reflection_tint" => &[Reflect0, Reflect1, Reflect2, Reflect3],
            _ => return Ok(None),
        };
        if fields.len() != args.len() {
            return Err(invalid(format!("{name}: wrong number of operands")));
        }
        if self.cinematic_base.is_none() {
            self.cinematic_base = Some(self.out.globals.len() as u16);
            for f in Field::ALL {
                self.cine_global(&f.name(), f.ty(), 0)?;
            }
        }
        let mut body = Vec::new();
        for (&f, &arg) in fields.iter().zip(args) {
            let value = self.expr(arg, f.ty())?;
            body.push(self.cine_set(f, value)?);
        }
        match name {
            "cinematic_screen_effect_start" => {
                let active = self.cine_var(Active);
                let inactive = self.control_call("not", h2t::BOOLEAN, &[active])?;
                let clear = self.cine_var(Clear);
                let reset = self.control_call("or", h2t::BOOLEAN, &[clear, inactive])?;
                let zero = [
                    Kernel, RadiusFrom, RadiusTo, LightFrom, LightTo, DesatFrom, DesatTo, Video,
                    Red, Green, Blue,
                ]
                .into_iter()
                .map(|f| self.cine_const(f, 0))
                .collect::<Result<Vec<_>>>()?;
                let clear_fields = self.control_call("begin", h2t::VOID, &zero)?;
                body.push(self.control_call("if", h2t::VOID, &[reset, clear_fields])?);
                body.push(self.cine_const(Active, 1)?);
                let clear = self.cine_var(Clear);
                body.push(self.control_call(name, h2t::VOID, &[clear])?);
            }
            "cinematic_screen_effect_stop" => {
                body.push(self.cine_const(Active, 0)?);
                body.push(self.control_call(name, h2t::VOID, &[])?);
            }
            "cinematic_screen_effect_set_convolution" | "cinematic_screen_effect_set_filter" => {
                body.push(self.cine_const(Video, 0)?);
                let tick = self.control_call("game_time", h2t::LONG, &[])?;
                body.push(self.cine_set(
                    if fields[0] == Passes {
                        BlurTick
                    } else {
                        FilterTick
                    },
                    tick,
                )?);
            }
            "cinematic_screen_effect_set_video" => {
                for f in [
                    Kernel, LightFrom, LightTo, DesatFrom, DesatTo, Red, Green, Blue,
                ] {
                    body.push(self.cine_const(f, 0)?);
                }
                body.push(self.cine_const(Video, 1)?);
            }
            _ => {}
        }
        self.control_call("begin", expected, &body).map(Some)
    }
}
