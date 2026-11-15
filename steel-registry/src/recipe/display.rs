//! Vanilla recipe-book displays: what the client's recipe book draws for a recipe.
//!
//! The server only ever encodes these, so the client-side `resolve` logic of vanilla
//! `SlotDisplay` has no counterpart here.

use std::io::{Result, Write};

use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

use crate::item_stack_template::ItemStackTemplate;
use crate::items::{Item, ItemRef};
use crate::trim_pattern::{TrimPattern, TrimPatternRef};
use crate::{RegistryEntry, RegistryHolder, RegistryHolderSet};

use super::properties::RecipeBookCategoryRef;
use super::{
    CookingRecipe, CraftingRecipe, Ingredient, SmithingRecipe, StonecuttingRecipe, TransmuteRecipe,
};
use crate::data_components::vanilla_components;
use crate::{REGISTRY, vanilla_items};

/// Vanilla `SlotDisplay`: the items one slot of a displayed recipe cycles through.
#[derive(Debug, Clone)]
pub enum SlotDisplay {
    /// Vanilla `SlotDisplay.Empty`.
    Empty,
    /// Vanilla `SlotDisplay.AnyFuel`.
    AnyFuel,
    /// Vanilla `SlotDisplay.WithAnyPotion`.
    WithAnyPotion(Box<Self>),
    /// Vanilla `SlotDisplay.OnlyWithComponent`, the component as its network id.
    OnlyWithComponent {
        /// The display filtered.
        source: Box<Self>,
        /// Network id of the data component a shown stack must carry.
        component: usize,
    },
    /// Vanilla `SlotDisplay.ItemSlotDisplay`.
    Item(ItemRef),
    /// Vanilla `SlotDisplay.ItemStackSlotDisplay`.
    ItemStack(ItemStackTemplate),
    /// Vanilla `SlotDisplay.TagSlotDisplay`.
    Tag(RegistryHolderSet<Item>),
    /// Vanilla `SlotDisplay.DyedSlotDemo`.
    Dyed {
        /// The dyes shown.
        dye: Box<Self>,
        /// The items dyed.
        target: Box<Self>,
    },
    /// Vanilla `SlotDisplay.SmithingTrimDemoSlotDisplay`.
    SmithingTrim {
        /// The items trimmed.
        base: Box<Self>,
        /// The trim materials.
        material: Box<Self>,
        /// The trim pattern applied.
        pattern: TrimPatternRef,
    },
    /// Vanilla `SlotDisplay.WithRemainder`.
    WithRemainder {
        /// The input shown.
        input: Box<Self>,
        /// What crafting leaves behind of it.
        remainder: Box<Self>,
    },
    /// Vanilla `SlotDisplay.Composite`.
    Composite(Vec<Self>),
}

impl SlotDisplay {
    /// Network id of this display's type, in vanilla `SlotDisplays.bootstrap` order.
    const fn type_id(&self) -> i32 {
        match self {
            Self::Empty => 0,
            Self::AnyFuel => 1,
            Self::WithAnyPotion(_) => 2,
            Self::OnlyWithComponent { .. } => 3,
            Self::Item(_) => 4,
            Self::ItemStack(_) => 5,
            Self::Tag(_) => 6,
            Self::Dyed { .. } => 7,
            Self::SmithingTrim { .. } => 8,
            Self::WithRemainder { .. } => 9,
            Self::Composite(_) => 10,
        }
    }

    /// Vanilla `Ingredient.display`; Steel's empty shaped-pattern cell is vanilla's
    /// absent ingredient, which `ShapedRecipe.display` shows as `SlotDisplay.Empty`.
    #[must_use]
    pub fn of_ingredient(ingredient: &Ingredient) -> Self {
        if ingredient.is_empty() {
            Self::Empty
        } else {
            Self::Tag(ingredient.contents())
        }
    }

    /// Vanilla `Ingredient.optionalIngredientToDisplay`.
    #[must_use]
    pub fn of_optional_ingredient(ingredient: Option<&Ingredient>) -> Self {
        ingredient.map_or(Self::Empty, Self::of_ingredient)
    }
}

impl WriteTo for SlotDisplay {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.type_id()).write(writer)?;
        match self {
            Self::Empty | Self::AnyFuel => Ok(()),
            Self::WithAnyPotion(display) => display.write(writer),
            Self::OnlyWithComponent { source, component } => {
                source.write(writer)?;
                write_id(*component, writer)
            }
            Self::Item(item) => write_id(item.id(), writer),
            Self::ItemStack(stack) => stack.write(writer),
            Self::Tag(items) => items.write(writer),
            Self::Dyed { dye, target } => {
                dye.write(writer)?;
                target.write(writer)
            }
            Self::SmithingTrim {
                base,
                material,
                pattern,
            } => {
                base.write(writer)?;
                material.write(writer)?;
                RegistryHolder::<TrimPattern>::reference(pattern).write(writer)
            }
            Self::WithRemainder { input, remainder } => {
                input.write(writer)?;
                remainder.write(writer)
            }
            Self::Composite(contents) => {
                write_len(contents.len(), writer)?;
                contents
                    .iter()
                    .try_for_each(|display| display.write(writer))
            }
        }
    }
}

/// Vanilla `RecipeDisplay`: how the recipe book lays a recipe out.
#[derive(Debug, Clone)]
pub enum RecipeDisplay {
    /// Vanilla `ShapelessCraftingRecipeDisplay`.
    Shapeless {
        /// One display per ingredient.
        ingredients: Vec<SlotDisplay>,
        /// The crafted result.
        result: SlotDisplay,
        /// The block the recipe is made at.
        crafting_station: SlotDisplay,
    },
    /// Vanilla `ShapedCraftingRecipeDisplay`.
    Shaped {
        /// Pattern width.
        width: usize,
        /// Pattern height.
        height: usize,
        /// `width * height` cells, row by row.
        ingredients: Vec<SlotDisplay>,
        /// The crafted result.
        result: SlotDisplay,
        /// The block the recipe is made at.
        crafting_station: SlotDisplay,
    },
    /// Vanilla `FurnaceRecipeDisplay`.
    Furnace {
        /// The smelted input.
        ingredient: SlotDisplay,
        /// The fuel shown.
        fuel: SlotDisplay,
        /// The cooked result.
        result: SlotDisplay,
        /// The furnace the recipe is made in.
        crafting_station: SlotDisplay,
        /// Cooking time in ticks.
        duration: i32,
        /// Experience the result gives.
        experience: f32,
    },
    /// Vanilla `StonecutterRecipeDisplay`.
    Stonecutter {
        /// The cut input.
        input: SlotDisplay,
        /// The result.
        result: SlotDisplay,
        /// The stonecutter.
        crafting_station: SlotDisplay,
    },
    /// Vanilla `SmithingRecipeDisplay`.
    Smithing {
        /// The template.
        template: SlotDisplay,
        /// The upgraded item.
        base: SlotDisplay,
        /// The added material.
        addition: SlotDisplay,
        /// The result.
        result: SlotDisplay,
        /// The smithing table.
        crafting_station: SlotDisplay,
    },
}

impl RecipeDisplay {
    /// Network id of this display's type, in vanilla `RecipeDisplays.bootstrap` order.
    const fn type_id(&self) -> i32 {
        match self {
            Self::Shapeless { .. } => 0,
            Self::Shaped { .. } => 1,
            Self::Furnace { .. } => 2,
            Self::Stonecutter { .. } => 3,
            Self::Smithing { .. } => 4,
        }
    }
}

impl WriteTo for RecipeDisplay {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.type_id()).write(writer)?;
        match self {
            Self::Shapeless {
                ingredients,
                result,
                crafting_station,
            } => {
                write_displays(ingredients, writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
            Self::Shaped {
                width,
                height,
                ingredients,
                result,
                crafting_station,
            } => {
                write_len(*width, writer)?;
                write_len(*height, writer)?;
                write_displays(ingredients, writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
            Self::Furnace {
                ingredient,
                fuel,
                result,
                crafting_station,
                duration,
                experience,
            } => {
                ingredient.write(writer)?;
                fuel.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)?;
                VarInt(*duration).write(writer)?;
                experience.write(writer)
            }
            Self::Stonecutter {
                input,
                result,
                crafting_station,
            } => {
                input.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
            Self::Smithing {
                template,
                base,
                addition,
                result,
                crafting_station,
            } => {
                template.write(writer)?;
                base.write(writer)?;
                addition.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
        }
    }
}

/// Vanilla `RecipeDisplayEntry`: one display of a recipe as the recipe book receives it.
#[derive(Debug, Clone)]
pub struct RecipeDisplayEntry {
    /// Vanilla `RecipeDisplayId`: the index of this display among all displays.
    pub id: usize,
    /// How the recipe is laid out.
    pub display: RecipeDisplay,
    /// Index of the recipe's group among all groups, if it has one.
    pub group: Option<usize>,
    /// The recipe-book tab the recipe shows in.
    pub category: RecipeBookCategoryRef,
    /// The ingredients the client checks to tell a craftable recipe; `None` for special
    /// recipes.
    pub crafting_requirements: Option<Vec<Ingredient>>,
}

impl WriteTo for RecipeDisplayEntry {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        write_len(self.id, writer)?;
        self.display.write(writer)?;
        self.group.is_some().write(writer)?;
        if let Some(group) = self.group {
            write_len(group, writer)?;
        }
        write_id(self.category.id(), writer)?;
        self.crafting_requirements.is_some().write(writer)?;
        if let Some(requirements) = &self.crafting_requirements {
            write_len(requirements.len(), writer)?;
            for ingredient in requirements {
                ingredient.contents().write(writer)?;
            }
        }
        Ok(())
    }
}

fn write_displays(displays: &[SlotDisplay], writer: &mut impl Write) -> Result<()> {
    write_len(displays.len(), writer)?;
    displays
        .iter()
        .try_for_each(|display| display.write(writer))
}

fn write_id(id: usize, writer: &mut impl Write) -> Result<()> {
    write_len(id, writer)
}

fn write_len(value: usize, writer: &mut impl Write) -> Result<()> {
    let value = i32::try_from(value)
        .map_err(|_| std::io::Error::other(format!("value is too large for a VarInt: {value}")))?;
    VarInt(value).write(writer)
}

impl CraftingRecipe {
    /// Vanilla `Recipe.display` of every crafting serializer; special recipes show none.
    #[must_use]
    pub fn displays(&self) -> Vec<RecipeDisplay> {
        let table = || SlotDisplay::Item(&vanilla_items::CRAFTING_TABLE);
        match self {
            Self::Shaped(recipe) => vec![RecipeDisplay::Shaped {
                width: recipe.width,
                height: recipe.height,
                ingredients: recipe
                    .pattern
                    .iter()
                    .map(SlotDisplay::of_ingredient)
                    .collect(),
                result: SlotDisplay::ItemStack(recipe.result.clone()),
                crafting_station: table(),
            }],
            Self::Shapeless(recipe) => vec![RecipeDisplay::Shapeless {
                ingredients: recipe
                    .ingredients
                    .iter()
                    .map(SlotDisplay::of_ingredient)
                    .collect(),
                result: SlotDisplay::ItemStack(recipe.result.clone()),
                crafting_station: table(),
            }],
            Self::Transmute(recipe) => transmute_displays(recipe),
            Self::Dye(recipe) => {
                let dyes = SlotDisplay::OnlyWithComponent {
                    source: Box::new(SlotDisplay::of_ingredient(&recipe.dye)),
                    component: REGISTRY
                        .data_components
                        .get_id(vanilla_components::DYE)
                        .expect("the dye component is registered"),
                };
                let target = SlotDisplay::of_ingredient(&recipe.target);
                vec![RecipeDisplay::Shapeless {
                    ingredients: vec![target.clone(), dyes.clone()],
                    result: SlotDisplay::Dyed {
                        dye: Box::new(dyes),
                        target: Box::new(target),
                    },
                    crafting_station: table(),
                }]
            }
            Self::Imbue(recipe) => {
                let material = SlotDisplay::of_ingredient(&recipe.material);
                let source = SlotDisplay::WithAnyPotion(Box::new(SlotDisplay::of_ingredient(
                    &recipe.source,
                )));
                let mut ingredients = vec![material; 9];
                ingredients[4] = source;
                vec![RecipeDisplay::Shaped {
                    width: 3,
                    height: 3,
                    ingredients,
                    result: SlotDisplay::WithAnyPotion(Box::new(SlotDisplay::ItemStack(
                        recipe.result.clone(),
                    ))),
                    crafting_station: table(),
                }]
            }
            Self::DecoratedPot(_)
            | Self::BannerDuplicate(_)
            | Self::BookCloning(_)
            | Self::FireworkRocket(_)
            | Self::FireworkStar(_)
            | Self::FireworkStarFade(_)
            | Self::MapExtending(_)
            | Self::RepairItem(_)
            | Self::ShieldDecoration(_) => Vec::new(),
        }
    }
}

/// Vanilla `TransmuteRecipe.display`: one shapeless display per material count.
fn transmute_displays(recipe: &TransmuteRecipe) -> Vec<RecipeDisplay> {
    let material = SlotDisplay::of_ingredient(&recipe.material);
    let mut ingredients = vec![SlotDisplay::of_ingredient(&recipe.input)];
    let base_count = recipe.result.as_ref().map_or(1, ItemStackTemplate::count);
    let mut displays = Vec::new();
    for material_count in recipe.min_material_count..=recipe.max_material_count {
        ingredients.push(material.clone());
        let count = if recipe.add_material_count_to_result {
            base_count + i32::try_from(material_count).unwrap_or(i32::MAX)
        } else {
            base_count
        };
        // Vanilla `resultDisplay`: without a result item, each input item transmuted.
        let result = recipe.result.as_ref().map_or_else(
            || {
                SlotDisplay::Composite(
                    recipe
                        .input
                        .get_items()
                        .into_iter()
                        .map(|item| {
                            SlotDisplay::ItemStack(ItemStackTemplate::with_count(item, count))
                        })
                        .collect(),
                )
            },
            |result| SlotDisplay::ItemStack(result.with_replaced_count(count)),
        );
        displays.push(RecipeDisplay::Shapeless {
            ingredients: ingredients.clone(),
            result,
            crafting_station: SlotDisplay::Item(&vanilla_items::CRAFTING_TABLE),
        });
    }
    displays
}

impl CookingRecipe {
    /// Vanilla `AbstractCookingRecipe.display`, made at `station`, the recipe type's
    /// `furnaceIcon`.
    #[must_use]
    pub fn displays(&self, station: ItemRef) -> Vec<RecipeDisplay> {
        vec![RecipeDisplay::Furnace {
            ingredient: SlotDisplay::of_ingredient(&self.ingredient),
            fuel: SlotDisplay::AnyFuel,
            result: SlotDisplay::ItemStack(self.result.clone()),
            crafting_station: SlotDisplay::Item(station),
            duration: self.cooking_time,
            experience: self.experience,
        }]
    }
}

impl StonecuttingRecipe {
    /// Vanilla `StonecutterRecipe.display`.
    #[must_use]
    pub fn displays(&self) -> Vec<RecipeDisplay> {
        vec![RecipeDisplay::Stonecutter {
            input: SlotDisplay::of_ingredient(&self.ingredient),
            result: SlotDisplay::ItemStack(self.result.clone()),
            crafting_station: SlotDisplay::Item(&vanilla_items::STONECUTTER),
        }]
    }
}

impl SmithingRecipe {
    /// Vanilla `SmithingTransformRecipe.display` and `SmithingTrimRecipe.display`.
    #[must_use]
    pub fn displays(&self) -> Vec<RecipeDisplay> {
        let table = SlotDisplay::Item(&vanilla_items::SMITHING_TABLE);
        match self {
            Self::Transform(recipe) => vec![RecipeDisplay::Smithing {
                template: SlotDisplay::of_optional_ingredient(recipe.template.as_ref()),
                base: SlotDisplay::of_ingredient(&recipe.base),
                addition: SlotDisplay::of_optional_ingredient(recipe.addition.as_ref()),
                result: SlotDisplay::ItemStack(recipe.result.clone()),
                crafting_station: table,
            }],
            Self::Trim(recipe) => {
                let base = SlotDisplay::of_ingredient(&recipe.base);
                let material = SlotDisplay::of_ingredient(&recipe.addition);
                vec![RecipeDisplay::Smithing {
                    template: SlotDisplay::of_ingredient(&recipe.template),
                    base: base.clone(),
                    addition: material.clone(),
                    result: SlotDisplay::SmithingTrim {
                        base: Box::new(base),
                        material: Box::new(material),
                        pattern: recipe.pattern,
                    },
                    crafting_station: table,
                }]
            }
        }
    }
}
