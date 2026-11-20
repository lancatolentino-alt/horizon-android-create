//! TerraFirmaCraft's `TFCAquifer` (EUPL 1.2): the water and lava pockets under the surface, with
//! sea level above it and lava at the bottom. A grid of jittered points each hold a fluid level; the
//! nearest three decide a block's fluid, and the pressure between them can wall a pocket in with
//! rock. `fluid_cell_noise` is TFC's `Cellular3D`.

use super::tfc_caves::CaveNoise;
use horizon_core::{
    position::BlockPos,
    random::{PositionalRandom, RandomSource, XoroshiroPositionalRandom, XoroshiroRandom},
};
use std::collections::HashMap;

const GRID_WIDTH: i32 = 16;
const GRID_HEIGHT: i32 = 12;
const XZ_RANGE: i32 = 10;
const Y_RANGE: i32 = 8;

/// What the aquifer says a non-solid block holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fluid {
    Air,
    Water,
    Lava,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    fluid: Fluid,
    y: i32,
}

impl Entry {
    fn at(&self, y: i32) -> Fluid {
        if y < self.y { self.fluid } else { Fluid::Air }
    }
}

const PRIME_X: i32 = 501125321;
const PRIME_Y: i32 = 1136930381;
const PRIME_Z: i32 = 1720413743;

/// `FastNoiseLite.RandVecs3D`.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
const RAND_VECS_3D: [f32; 1024] = [
    -0.7292736885, -0.6618439697, 0.1735581948, 0.0, 0.790292081, -0.5480887466, -0.2739291014, 0.0,
    0.7217578935, 0.6226212466, -0.3023380997, 0.0, 0.565683137, -0.8208298145, -0.0790000257, 0.0,
    0.760049034, -0.5555979497, -0.3370999617, 0.0, 0.3713945616, 0.5011264475, 0.7816254623, 0.0,
    -0.1277062463, -0.4254438999, -0.8959289049, 0.0, -0.2881560924, -0.5815838982, 0.7607405838, 0.0,
    0.5849561111, -0.662820239, -0.4674352136, 0.0, 0.3307171178, 0.0391653737, 0.94291689, 0.0,
    0.8712121778, -0.4113374369, -0.2679381538, 0.0, 0.580981015, 0.7021915846, 0.4115677815, 0.0,
    0.503756873, 0.6330056931, -0.5878203852, 0.0, 0.4493712205, 0.601390195, 0.6606022552, 0.0,
    -0.6878403724, 0.09018890807, -0.7202371714, 0.0, -0.5958956522, -0.6469350577, 0.475797649, 0.0,
    -0.5127052122, 0.1946921978, -0.8361987284, 0.0, -0.9911507142, -0.05410276466, -0.1212153153, 0.0,
    -0.2149721042, 0.9720882117, -0.09397607749, 0.0, -0.7518650936, -0.5428057603, 0.3742469607, 0.0,
    0.5237068895, 0.8516377189, -0.02107817834, 0.0, 0.6333504779, 0.1926167129, -0.7495104896, 0.0,
    -0.06788241606, 0.3998305789, 0.9140719259, 0.0, -0.5538628599, -0.4729896695, -0.6852128902, 0.0,
    -0.7261455366, -0.5911990757, 0.3509933228, 0.0, -0.9229274737, -0.1782808786, 0.3412049336, 0.0,
    -0.6968815002, 0.6511274338, 0.3006480328, 0.0, 0.9608044783, -0.2098363234, -0.1811724921, 0.0,
    0.06817146062, -0.9743405129, 0.2145069156, 0.0, -0.3577285196, -0.6697087264, -0.6507845481, 0.0,
    -0.1868621131, 0.7648617052, -0.6164974636, 0.0, -0.6541697588, 0.3967914832, 0.6439087246, 0.0,
    0.6993340405, -0.6164538506, 0.3618239211, 0.0, -0.1546665739, 0.6291283928, 0.7617583057, 0.0,
    -0.6841612949, -0.2580482182, -0.6821542638, 0.0, 0.5383980957, 0.4258654885, 0.7271630328, 0.0,
    -0.5026987823, -0.7939832935, -0.3418836993, 0.0, 0.3202971715, 0.2834415347, 0.9039195862, 0.0,
    0.8683227101, -0.0003762656404, -0.4959995258, 0.0, 0.791120031, -0.08511045745, 0.6057105799, 0.0,
    -0.04011016052, -0.4397248749, 0.8972364289, 0.0, 0.9145119872, 0.3579346169, -0.1885487608, 0.0,
    -0.9612039066, -0.2756484276, 0.01024666929, 0.0, 0.6510361721, -0.2877799159, -0.7023778346, 0.0,
    -0.2041786351, 0.7365237271, 0.644859585, 0.0, -0.7718263711, 0.3790626912, 0.5104855816, 0.0,
    -0.3060082741, -0.7692987727, 0.5608371729, 0.0, 0.454007341, -0.5024843065, 0.7357899537, 0.0,
    0.4816795475, 0.6021208291, -0.6367380315, 0.0, 0.6961980369, -0.3222197429, 0.641469197, 0.0,
    -0.6532160499, -0.6781148932, 0.3368515753, 0.0, 0.5089301236, -0.6154662304, -0.6018234363, 0.0,
    -0.1635919754, -0.9133604627, -0.372840892, 0.0, 0.52408019, -0.8437664109, 0.1157505864, 0.0,
    0.5902587356, 0.4983817807, -0.6349883666, 0.0, 0.5863227872, 0.494764745, 0.6414307729, 0.0,
    0.6779335087, 0.2341345225, 0.6968408593, 0.0, 0.7177054546, -0.6858979348, 0.120178631, 0.0,
    -0.5328819713, -0.5205125012, 0.6671608058, 0.0, -0.8654874251, -0.0700727088, -0.4960053754, 0.0,
    -0.2861810166, 0.7952089234, 0.5345495242, 0.0, -0.04849529634, 0.9810836427, -0.1874115585, 0.0,
    -0.6358521667, 0.6058348682, 0.4781800233, 0.0, 0.6254794696, -0.2861619734, 0.7258696564, 0.0,
    -0.2585259868, 0.5061949264, -0.8227581726, 0.0, 0.02136306781, 0.5064016808, -0.8620330371, 0.0,
    0.200111773, 0.8599263484, 0.4695550591, 0.0, 0.4743561372, 0.6014985084, -0.6427953014, 0.0,
    0.6622993731, -0.5202474575, -0.5391679918, 0.0, 0.08084972818, -0.6532720452, 0.7527940996, 0.0,
    -0.6893687501, 0.0592860349, 0.7219805347, 0.0, -0.1121887082, -0.9673185067, 0.2273952515, 0.0,
    0.7344116094, 0.5979668656, -0.3210532909, 0.0, 0.5789393465, -0.2488849713, 0.7764570201, 0.0,
    0.6988182827, 0.3557169806, -0.6205791146, 0.0, -0.8636845529, -0.2748771249, -0.4224826141, 0.0,
    -0.4247027957, -0.4640880967, 0.777335046, 0.0, 0.5257722489, -0.8427017621, 0.1158329937, 0.0,
    0.9343830603, 0.316302472, -0.1639543925, 0.0, -0.1016836419, -0.8057303073, -0.5834887393, 0.0,
    -0.6529238969, 0.50602126, -0.5635892736, 0.0, -0.2465286165, -0.9668205684, -0.06694497494, 0.0,
    -0.9776897119, -0.2099250524, -0.007368825344, 0.0, 0.7736893337, 0.5734244712, 0.2694238123, 0.0,
    -0.6095087895, 0.4995678998, 0.6155736747, 0.0, 0.5794535482, 0.7434546771, 0.3339292269, 0.0,
    -0.8226211154, 0.08142581855, 0.5627293636, 0.0, -0.510385483, 0.4703667658, 0.7199039967, 0.0,
    -0.5764971849, -0.07231656274, -0.8138926898, 0.0, 0.7250628871, 0.3949971505, -0.5641463116, 0.0,
    -0.1525424005, 0.4860840828, -0.8604958341, 0.0, -0.5550976208, -0.4957820792, 0.667882296, 0.0,
    -0.1883614327, 0.9145869398, 0.357841725, 0.0, 0.7625556724, -0.5414408243, -0.3540489801, 0.0,
    -0.5870231946, -0.3226498013, -0.7424963803, 0.0, 0.3051124198, 0.2262544068, -0.9250488391, 0.0,
    0.6379576059, 0.577242424, -0.5097070502, 0.0, -0.5966775796, 0.1454852398, -0.7891830656, 0.0,
    -0.658330573, 0.6555487542, -0.3699414651, 0.0, 0.7434892426, 0.2351084581, 0.6260573129, 0.0,
    0.5562114096, 0.8264360377, -0.0873632843, 0.0, -0.3028940016, -0.8251527185, 0.4768419182, 0.0,
    0.1129343818, -0.985888439, -0.1235710781, 0.0, 0.5937652891, -0.5896813806, 0.5474656618, 0.0,
    0.6757964092, -0.5835758614, -0.4502648413, 0.0, 0.7242302609, -0.1152719764, 0.6798550586, 0.0,
    -0.9511914166, 0.0753623979, -0.2992580792, 0.0, 0.2539470961, -0.1886339355, 0.9486454084, 0.0,
    0.571433621, -0.1679450851, -0.8032795685, 0.0, -0.06778234979, 0.3978269256, 0.9149531629, 0.0,
    0.6074972649, 0.733060024, -0.3058922593, 0.0, -0.5435478392, 0.1675822484, 0.8224791405, 0.0,
    -0.5876678086, -0.3380045064, -0.7351186982, 0.0, -0.7967562402, 0.04097822706, -0.6029098428, 0.0,
    -0.1996350917, 0.8706294745, 0.4496111079, 0.0, -0.02787660336, -0.9106232682, -0.4122962022, 0.0,
    -0.7797625996, -0.6257634692, 0.01975775581, 0.0, -0.5211232846, 0.7401644346, -0.4249554471, 0.0,
    0.8575424857, 0.4053272873, -0.3167501783, 0.0, 0.1045223322, 0.8390195772, -0.5339674439, 0.0,
    0.3501822831, 0.9242524096, -0.1520850155, 0.0, 0.1987849858, 0.07647613266, 0.9770547224, 0.0,
    0.7845996363, 0.6066256811, -0.1280964233, 0.0, 0.09006737436, -0.9750989929, -0.2026569073, 0.0,
    -0.8274343547, -0.542299559, 0.1458203587, 0.0, -0.3485797732, -0.415802277, 0.840000362, 0.0,
    -0.2471778936, -0.7304819962, -0.6366310879, 0.0, -0.3700154943, 0.8577948156, 0.3567584454, 0.0,
    0.5913394901, -0.548311967, -0.5913303597, 0.0, 0.1204873514, -0.7626472379, -0.6354935001, 0.0,
    0.616959265, 0.03079647928, 0.7863922953, 0.0, 0.1258156836, -0.6640829889, -0.7369967419, 0.0,
    -0.6477565124, -0.1740147258, -0.7417077429, 0.0, 0.6217889313, -0.7804430448, -0.06547655076, 0.0,
    0.6589943422, -0.6096987708, 0.4404473475, 0.0, -0.2689837504, -0.6732403169, -0.6887635427, 0.0,
    -0.3849775103, 0.5676542638, 0.7277093879, 0.0, 0.5754444408, 0.8110471154, -0.1051963504, 0.0,
    0.9141593684, 0.3832947817, 0.131900567, 0.0, -0.107925319, 0.9245493968, 0.3654593525, 0.0,
    0.377977089, 0.3043148782, 0.8743716458, 0.0, -0.2142885215, -0.8259286236, 0.5214617324, 0.0,
    0.5802544474, 0.4148098596, -0.7008834116, 0.0, -0.1982660881, 0.8567161266, -0.4761596756, 0.0,
    -0.03381553704, 0.3773180787, -0.9254661404, 0.0, -0.6867922841, -0.6656597827, 0.2919133642, 0.0,
    0.7731742607, -0.2875793547, -0.5652430251, 0.0, -0.09655941928, 0.9193708367, -0.3813575004, 0.0,
    0.2715702457, -0.9577909544, -0.09426605581, 0.0, 0.2451015704, -0.6917998565, -0.6792188003, 0.0,
    0.977700782, -0.1753855374, 0.1155036542, 0.0, -0.5224739938, 0.8521606816, 0.02903615945, 0.0,
    -0.7734880599, -0.5261292347, 0.3534179531, 0.0, -0.7134492443, -0.269547243, 0.6467878011, 0.0,
    0.1644037271, 0.5105846203, -0.8439637196, 0.0, 0.6494635788, 0.05585611296, 0.7583384168, 0.0,
    -0.4711970882, 0.5017280509, -0.7254255765, 0.0, -0.6335764307, -0.2381686273, -0.7361091029, 0.0,
    -0.9021533097, -0.270947803, -0.3357181763, 0.0, -0.3793711033, 0.872258117, 0.3086152025, 0.0,
    -0.6855598966, -0.3250143309, 0.6514394162, 0.0, 0.2900942212, -0.7799057743, -0.5546100667, 0.0,
    -0.2098319339, 0.85037073, 0.4825351604, 0.0, -0.4592603758, 0.6598504336, -0.5947077538, 0.0,
    0.8715945488, 0.09616365406, -0.4807031248, 0.0, -0.6776666319, 0.7118504878, -0.1844907016, 0.0,
    0.7044377633, 0.312427597, 0.637304036, 0.0, -0.7052318886, -0.2401093292, -0.6670798253, 0.0,
    0.081921007, -0.7207336136, -0.6883545647, 0.0, -0.6993680906, -0.5875763221, -0.4069869034, 0.0,
    -0.1281454481, 0.6419895885, 0.7559286424, 0.0, -0.6337388239, -0.6785471501, -0.3714146849, 0.0,
    0.5565051903, -0.2168887573, -0.8020356851, 0.0, -0.5791554484, 0.7244372011, -0.3738578718, 0.0,
    0.1175779076, -0.7096451073, 0.6946792478, 0.0, -0.6134619607, 0.1323631078, 0.7785527795, 0.0,
    0.6984635305, -0.02980516237, -0.715024719, 0.0, 0.8318082963, -0.3930171956, 0.3919597455, 0.0,
    0.1469576422, 0.05541651717, -0.9875892167, 0.0, 0.708868575, -0.2690503865, 0.6520101478, 0.0,
    0.2726053183, 0.67369766, -0.68688995, 0.0, -0.6591295371, 0.3035458599, -0.6880466294, 0.0,
    0.4815131379, -0.7528270071, 0.4487723203, 0.0, 0.9430009463, 0.1675647412, -0.2875261255, 0.0,
    0.434802957, 0.7695304522, -0.4677277752, 0.0, 0.3931996188, 0.594473625, 0.7014236729, 0.0,
    0.7254336655, -0.603925654, 0.3301814672, 0.0, 0.7590235227, -0.6506083235, 0.02433313207, 0.0,
    -0.8552768592, -0.3430042733, 0.3883935666, 0.0, -0.6139746835, 0.6981725247, 0.3682257648, 0.0,
    -0.7465905486, -0.5752009504, 0.3342849376, 0.0, 0.5730065677, 0.810555537, -0.1210916791, 0.0,
    -0.9225877367, -0.3475211012, -0.167514036, 0.0, -0.7105816789, -0.4719692027, -0.5218416899, 0.0,
    -0.08564609717, 0.3583001386, 0.929669703, 0.0, -0.8279697606, -0.2043157126, 0.5222271202, 0.0,
    0.427944023, 0.278165994, 0.8599346446, 0.0, 0.5399079671, -0.7857120652, -0.3019204161, 0.0,
    0.5678404253, -0.5495413974, -0.6128307303, 0.0, -0.9896071041, 0.1365639107, -0.04503418428, 0.0,
    -0.6154342638, -0.6440875597, 0.4543037336, 0.0, 0.1074204368, -0.7946340692, 0.5975094525, 0.0,
    -0.3595449969, -0.8885529948, 0.28495784, 0.0, -0.2180405296, 0.1529888965, 0.9638738118, 0.0,
    -0.7277432317, -0.6164050508, -0.3007234646, 0.0, 0.7249729114, -0.00669719484, 0.6887448187, 0.0,
    -0.5553659455, -0.5336586252, 0.6377908264, 0.0, 0.5137558015, 0.7976208196, -0.3160000073, 0.0,
    -0.3794024848, 0.9245608561, -0.03522751494, 0.0, 0.8229248658, 0.2745365933, -0.4974176556, 0.0,
    -0.5404114394, 0.6091141441, 0.5804613989, 0.0, 0.8036581901, -0.2703029469, 0.5301601931, 0.0,
    0.6044318879, 0.6832968393, 0.4095943388, 0.0, 0.06389988817, 0.9658208605, -0.2512108074, 0.0,
    0.1087113286, 0.7402471173, -0.6634877936, 0.0, -0.713427712, -0.6926784018, 0.1059128479, 0.0,
    0.6458897819, -0.5724548511, -0.5050958653, 0.0, -0.6553931414, 0.7381471625, 0.159995615, 0.0,
    0.3910961323, 0.9188871375, -0.05186755998, 0.0, -0.4879022471, -0.5904376907, 0.6429111375, 0.0,
    0.6014790094, 0.7707441366, -0.2101820095, 0.0, -0.5677173047, 0.7511360995, 0.3368851762, 0.0,
    0.7858573506, 0.226674665, 0.5753666838, 0.0, -0.4520345543, -0.604222686, -0.6561857263, 0.0,
    0.002272116345, 0.4132844051, -0.9105991643, 0.0, -0.5815751419, -0.5162925989, 0.6286591339, 0.0,
    -0.03703704785, 0.8273785755, 0.5604221175, 0.0, -0.5119692504, 0.7953543429, -0.3244980058, 0.0,
    -0.2682417366, -0.9572290247, -0.1084387619, 0.0, -0.2322482736, -0.9679131102, -0.09594243324, 0.0,
    0.3554328906, -0.8881505545, 0.2913006227, 0.0, 0.7346520519, -0.4371373164, 0.5188422971, 0.0,
    0.9985120116, 0.04659011161, -0.02833944577, 0.0, -0.3727687496, -0.9082481361, 0.1900757285, 0.0,
    0.91737377, -0.3483642108, 0.1925298489, 0.0, 0.2714911074, 0.4147529736, -0.8684886582, 0.0,
    0.5131763485, -0.7116334161, 0.4798207128, 0.0, -0.8737353606, 0.18886992, -0.4482350644, 0.0,
    0.8460043821, -0.3725217914, 0.3814499973, 0.0, 0.8978727456, -0.1780209141, -0.4026575304, 0.0,
    0.2178065647, -0.9698322841, -0.1094789531, 0.0, -0.1518031304, -0.7788918132, -0.6085091231, 0.0,
    -0.2600384876, -0.4755398075, -0.8403819825, 0.0, 0.572313509, -0.7474340931, -0.3373418503, 0.0,
    -0.7174141009, 0.1699017182, -0.6756111411, 0.0, -0.684180784, 0.02145707593, -0.7289967412, 0.0,
    -0.2007447902, 0.06555605789, -0.9774476623, 0.0, -0.1148803697, -0.8044887315, 0.5827524187, 0.0,
    -0.7870349638, 0.03447489231, 0.6159443543, 0.0, -0.2015596421, 0.6859872284, 0.6991389226, 0.0,
    -0.08581082512, -0.10920836, -0.9903080513, 0.0, 0.5532693395, 0.7325250401, -0.396610771, 0.0,
    -0.1842489331, -0.9777375055, -0.1004076743, 0.0, 0.0775473789, -0.9111505856, 0.4047110257, 0.0,
    0.1399838409, 0.7601631212, -0.6344734459, 0.0, 0.4484419361, -0.845289248, 0.2904925424, 0.0,
];

fn fast_round(f: f64) -> i32 {
    if f >= 0.0 {
        (f + f64::from(0.5f32)) as i32
    } else {
        (f - f64::from(0.5f32)) as i32
    }
}

/// TFC's `Cellular3D`: the nearest jittered point, its height and its hash value.
struct Cellular3D {
    seed: i32,
    frequency: f64,
}

struct Cell {
    y: f64,
    noise: f64,
}

impl Cellular3D {
    fn new(seed: i64) -> Self {
        // `HashCommon.long2int`.
        let seed = (seed ^ (seed >> 32)) as i32;
        Self {
            seed,
            frequency: 1.0,
        }
    }

    fn spread(mut self, scale: f64) -> Self {
        self.frequency *= scale;
        self
    }

    fn cell(&self, x: f64, y: f64, z: f64) -> Cell {
        let (x, y, z) = (x * self.frequency, y * self.frequency, z * self.frequency);
        let (xr, yr, zr) = (fast_round(x), fast_round(y), fast_round(z));
        let mut distance0 = f64::MAX;
        let mut distance1 = f64::MAX;
        let mut closest_hash = 0i32;
        let mut closest_y = 0.0f64;
        let jitter = f64::from(0.39614353f32);
        let mut x_primed = xr.wrapping_sub(1).wrapping_mul(PRIME_X);
        let y_primed_base = yr.wrapping_sub(1).wrapping_mul(PRIME_Y);
        let z_primed_base = zr.wrapping_sub(1).wrapping_mul(PRIME_Z);
        for xi in (xr - 1)..=(xr + 1) {
            let mut y_primed = y_primed_base;
            for yi in (yr - 1)..=(yr + 1) {
                let mut z_primed = z_primed_base;
                for zi in (zr - 1)..=(zr + 1) {
                    let hash =
                        (self.seed ^ x_primed ^ y_primed ^ z_primed).wrapping_mul(0x27d4eb2d);
                    let idx = (hash & (255 << 2)) as usize;
                    let vec_x = f64::from(xi) + f64::from(RAND_VECS_3D[idx]) * jitter;
                    let vec_y = f64::from(yi) + f64::from(RAND_VECS_3D[idx | 1]) * jitter;
                    let vec_z = f64::from(zi) + f64::from(RAND_VECS_3D[idx | 2]) * jitter;
                    let distance = (vec_x - x) * (vec_x - x)
                        + (vec_y - y) * (vec_y - y)
                        + (vec_z - z) * (vec_z - z);
                    distance1 = distance1.min(distance).max(distance0);
                    if distance < distance0 {
                        distance0 = distance;
                        closest_hash = hash;
                        closest_y = vec_y;
                    }
                    z_primed = z_primed.wrapping_add(PRIME_Z);
                }
                y_primed = y_primed.wrapping_add(PRIME_Y);
            }
            x_primed = x_primed.wrapping_add(PRIME_X);
        }
        Cell {
            y: closest_y / self.frequency,
            noise: f64::from(closest_hash as f32 * (1.0f32 / 2147483648.0f32)),
        }
    }
}

/// `similarity(firstDistance, secondDistance)`.
fn similarity(first: i32, second: i32) -> f64 {
    1.0 - f64::from((second - first).abs()) / 25.0
}

pub struct TfcAquifer<'a> {
    min_chunk_x: i32,
    min_chunk_z: i32,
    min_y: i32,
    fork: XoroshiroPositionalRandom,
    caves: &'a CaveNoise,
    fluid_cell_seed: i64,
    fluid_cell_noise: Cellular3D,
    lava_level: Entry,
    sea_level: Entry,
    /// The 4x4 chunk-resolution aquifer surface heights, indexed `x + 4 * z`.
    surface_heights: [i32; 16],
    locations: HashMap<(i32, i32, i32), [i32; 3]>,
    entries: HashMap<(i32, i32, i32), Entry>,
}

impl<'a> TfcAquifer<'a> {
    pub fn new(
        chunk_x: i32,
        chunk_z: i32,
        min_y: i32,
        sea_level: i32,
        caves: &'a CaveNoise,
        surface_heights: [i32; 16],
    ) -> Self {
        let fork = caves.fork();
        let mut cell_random = fork.from_hash_of("aquifer_fluid_cell_noise");
        let fluid_cell_seed = cell_random.next_long();
        let fluid_cell_noise = Cellular3D::new(cell_random.next_long()).spread(f64::from(0.015f32));
        Self {
            min_chunk_x: chunk_x << 4,
            min_chunk_z: chunk_z << 4,
            min_y,
            fork,
            caves,
            fluid_cell_seed,
            fluid_cell_noise,
            lava_level: Entry {
                fluid: Fluid::Lava,
                y: min_y + 10,
            },
            sea_level: Entry {
                fluid: Fluid::Water,
                y: sea_level,
            },
            surface_heights,
            locations: HashMap::new(),
            entries: HashMap::new(),
        }
    }

    fn global(&self, y: i32) -> Entry {
        if y < self.min_y + 10 {
            self.lava_level
        } else {
            self.sea_level
        }
    }

    fn location(&mut self, gx: i32, gy: i32, gz: i32) -> [i32; 3] {
        if let Some(found) = self.locations.get(&(gx, gy, gz)) {
            return *found;
        }
        let mut random = self.fork.at(BlockPos::new(gx, gy, gz));
        let mut next = |bound: i32| random.next_int_bound(bound).unwrap_or(0);
        let x = gx * GRID_WIDTH + next(XZ_RANGE) + (GRID_WIDTH - XZ_RANGE) / 2;
        let y = gy * GRID_HEIGHT + next(Y_RANGE) + (GRID_HEIGHT - Y_RANGE) / 2;
        let z = gz * GRID_WIDTH + next(XZ_RANGE) + (GRID_WIDTH - XZ_RANGE) / 2;
        self.locations.insert((gx, gy, gz), [x, y, z]);
        [x, y, z]
    }

    fn entry(&mut self, at: [i32; 3]) -> Entry {
        let key = (
            at[0].div_euclid(GRID_WIDTH),
            at[1].div_euclid(GRID_HEIGHT),
            at[2].div_euclid(GRID_WIDTH),
        );
        if let Some(found) = self.entries.get(&key) {
            return *found;
        }
        let made = self.create(at[0], at[1], at[2]);
        self.entries.insert(key, made);
        made
    }

    /// `createAquifer`.
    fn create(&self, x: i32, y: i32, z: i32) -> Entry {
        let (dx, dz) = (x - self.min_chunk_x + 16, z - self.min_chunk_z + 16);
        let index = ((dx >> 4) + 4 * (dz >> 4)).clamp(0, 15) as usize;
        let surface = self.surface_heights[index];
        if y >= surface {
            return self.sea_level;
        }
        let cell =
            self.fluid_cell_noise
                .cell(f64::from(x), f64::from(y as f32 / 0.6f32), f64::from(z));
        let cell_noise = cell.noise as f32;
        let cell_y = cell.y as f32;
        if cell_noise < 0.25 || (cell_y > 53.0 && cell_noise < 0.5) {
            return Entry {
                fluid: Fluid::Water,
                y: self.min_y - 1,
            };
        }
        let mut random = XoroshiroRandom::from_state(
            self.fluid_cell_seed as u64,
            i64::from(cell_noise.to_bits() as i32) as u64,
        );
        let a = random.next_float();
        let b = random.next_float();
        let aquifer_y = ((a - b - 2.0) * 5.0 + cell_y).min(surface as f32);
        let lava = cell_y < 40.0 && random.next_int_bound(3).unwrap_or(1) == 0;
        Entry {
            fluid: if lava { Fluid::Lava } else { Fluid::Water },
            y: aquifer_y as i32,
        }
    }

    fn pressure(
        &self,
        x: i32,
        y: i32,
        z: i32,
        barrier: &mut Option<f64>,
        left: Entry,
        right: Entry,
    ) -> f64 {
        let (left_state, right_state) = (left.at(y), right.at(y));
        if (left_state == Fluid::Lava && right_state == Fluid::Water)
            || (left_state == Fluid::Water && right_state == Fluid::Lava)
        {
            return 1.0;
        }
        let delta_level = (left.y - right.y).abs();
        if delta_level == 0 {
            return 0.0;
        }
        let average = 0.5 * f64::from(left.y + right.y);
        let above_average = f64::from(y) + 0.5 - average;
        let near_average = 0.5 * f64::from(delta_level) - above_average.abs();
        let pressure = if above_average > 0.0 {
            near_average / if near_average > 0.0 { 1.5 } else { 2.5 }
        } else {
            (near_average + 3.0) / if near_average > -3.0 { 3.0 } else { 10.0 }
        };
        if !(-2.0..=2.0).contains(&pressure) {
            return pressure;
        }
        let value = match *barrier {
            Some(value) => value,
            None => {
                let value = self
                    .caves
                    .barrier_at(f64::from(x), f64::from(y) * 0.5, f64::from(z));
                *barrier = Some(value);
                value
            }
        };
        value + pressure
    }

    /// `sampleState`: `terrain_noise` is the density, positive for solid. `None` means the block
    /// stays rock (solid terrain, or a barrier between pockets); `Some` is what fills it.
    pub fn sample_state(&mut self, x: i32, y: i32, z: i32, terrain_noise: f64) -> Option<Fluid> {
        if terrain_noise > 0.0 {
            return None;
        }
        let global = self.global(y);
        let (state, contribution);
        if global.at(y) == Fluid::Lava {
            state = Fluid::Lava;
            contribution = 0.0;
        } else {
            let lower_x = (x - XZ_RANGE / 2).div_euclid(GRID_WIDTH);
            let lower_y = y.div_euclid(GRID_HEIGHT);
            let lower_z = (z - XZ_RANGE / 2).div_euclid(GRID_WIDTH);
            let (mut d1, mut d2, mut d3) = (i32::MAX, i32::MAX, i32::MAX);
            let (mut a1, mut a2, mut a3) = ([0; 3], [0; 3], [0; 3]);
            for ox in 0..=1 {
                for oy in -1..=1 {
                    for oz in 0..=1 {
                        let at = self.location(lower_x + ox, lower_y + oy, lower_z + oz);
                        let (dx, dy, dz) = (at[0] - x, at[1] - y, at[2] - z);
                        let distance =
                            dx.wrapping_mul(dx) + dy.wrapping_mul(dy) + dz.wrapping_mul(dz);
                        if distance <= d1 {
                            a3 = a2;
                            a2 = a1;
                            a1 = at;
                            d3 = d2;
                            d2 = d1;
                            d1 = distance;
                        } else if distance <= d2 {
                            a3 = a2;
                            a2 = at;
                            d3 = d2;
                            d2 = distance;
                        } else if distance <= d3 {
                            a3 = at;
                            d3 = distance;
                        }
                    }
                }
            }
            let (e1, e2, e3) = (self.entry(a1), self.entry(a2), self.entry(a3));
            let (s12, s13, s23) = (similarity(d1, d2), similarity(d1, d3), similarity(d2, d3));
            if e1.at(y) == Fluid::Water && self.global(y - 1).at(y - 1) == Fluid::Lava {
                contribution = 1.0;
            } else if s12 > -1.0 {
                let mut barrier = None;
                let p12 = self.pressure(x, y, z, &mut barrier, e1, e2);
                let p13 = self.pressure(x, y, z, &mut barrier, e1, e3);
                let p23 = self.pressure(x, y, z, &mut barrier, e2, e3);
                let (c12, c13, c23) = (s12.max(0.0), s13.max(0.0), s23.max(0.0));
                contribution = (2.0 * c12 * p12.max((p13 * c13).max(p23 * c23))).max(0.0);
            } else {
                contribution = 0.0;
            }
            state = e1.at(y);
        }
        (terrain_noise + contribution <= 0.0).then_some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_terrain_stays_rock_and_the_bottom_is_lava() {
        let caves = CaveNoise::new(99);
        let mut aquifer = TfcAquifer::new(0, 0, -64, 63, &caves, [60; 16]);
        assert_eq!(aquifer.sample_state(3, 20, 4, 0.2), None);
        assert_eq!(aquifer.sample_state(3, -60, 4, -0.5), Some(Fluid::Lava));
    }

    #[test]
    fn open_water_below_the_surface_aquifer_is_sea_level_water() {
        let caves = CaveNoise::new(99);
        // Every aquifer cell is above the surface here, so the sea level entry rules everywhere.
        let mut aquifer = TfcAquifer::new(2, -3, -64, 63, &caves, [-100; 16]);
        for (x, z) in [(40, -40), (45, -35), (47, -33)] {
            assert_eq!(aquifer.sample_state(x, 40, z, -1.0), Some(Fluid::Water));
            assert_eq!(aquifer.sample_state(x, 70, z, -1.0), Some(Fluid::Air));
        }
    }

    #[test]
    fn deep_pockets_hold_water_lava_air_and_barrier_rock() {
        let caves = CaveNoise::new(2024);
        let mut aquifer = TfcAquifer::new(0, 0, -64, 63, &caves, [120; 16]);
        let mut seen = [0usize; 4];
        for x in 0..16 {
            for z in 0..16 {
                for y in -64..100 {
                    let slot = match aquifer.sample_state(x, y, z, -0.01) {
                        Some(Fluid::Air) => 0,
                        Some(Fluid::Water) => 1,
                        Some(Fluid::Lava) => 2,
                        None => 3,
                    };
                    seen[slot] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
        // The same inputs always give the same answer.
        let mut again = TfcAquifer::new(0, 0, -64, 63, &caves, [120; 16]);
        assert_eq!(
            aquifer.sample_state(5, 0, 5, -0.01),
            again.sample_state(5, 0, 5, -0.01)
        );
    }
}
