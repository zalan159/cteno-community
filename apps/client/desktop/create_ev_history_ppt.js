const pptx = require('pptxgenjs');

// 创建演示文稿
const pres = new pptx();

// 设置演示文稿属性
pres.title = '新能源汽车发展历史';
pres.author = 'Cteno AI Assistant';

// 定义颜色主题
const colors = {
  primary: '028090',   // 青绿
  secondary: '00A896', // 海沫绿
  accent: '02C39A',    // 薄荷绿
  dark: '212121',      // 深灰
  light: 'F2F2F2'      // 浅灰
};

// 定义幻灯片母版
pres.defineSlideMaster({
  title: 'MASTER',
  background: { color: 'FFFFFF' },
  objects: [
    // 标题占位符
    { placeholder: { options: { name: 'title', type: 'title', x: 0.5, y: 0.3, w: 9, h: 1.0 }, text: '标题' } },
    // 内容占位符
    { placeholder: { options: { name: 'body', type: 'body', x: 0.5, y: 1.5, w: 9, h: 5.0 }, text: '内容' } },
    // 页脚
    { text: { text: '新能源汽车发展历史', options: { x: 0.5, y: 6.8, w: 9, h: 0.3, fontSize: 10, color: colors.dark, align: 'center' } } },
    { text: { text: '第 {slideNum} 页', options: { x: 0.5, y: 7.0, w: 9, h: 0.3, fontSize: 10, color: colors.dark, align: 'center' } } }
  ]
});

// 1. 标题页
let slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('新能源汽车发展历史', {
  x: 0.5, y: 2.0, w: 9, h: 1.5,
  fontSize: 44,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia',
  align: 'center'
});
slide.addText('从概念到主流的演进之路', {
  x: 0.5, y: 3.5, w: 9, h: 0.8,
  fontSize: 24,
  color: colors.secondary,
  fontFace: 'Calibri',
  align: 'center',
  italic: true
});
slide.addText('Cteno AI Assistant | ' + new Date().toLocaleDateString('zh-CN'), {
  x: 0.5, y: 6.5, w: 9, h: 0.4,
  fontSize: 14,
  color: colors.dark,
  fontFace: 'Calibri',
  align: 'center'
});

// 2. 目录页
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('目录', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia',
  align: 'center'
});

const contents = [
  '早期探索 (19世纪-20世纪初)',
  '沉寂期 (20世纪中期)',
  '复苏与技术进步 (20世纪末)',
  '21世纪爆发 (特斯拉引领)',
  '中国新能源汽车发展',
  '当前趋势与未来展望'
];

let yStart = 1.8;
contents.forEach((item, idx) => {
  slide.addText(`${idx + 1}. ${item}`, {
    x: 1.0, y: yStart + idx * 0.7, w: 8, h: 0.6,
    fontSize: 20,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: { type: 'number' }
  });
});

// 3. 早期探索
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('早期探索 (19世纪-20世纪初)', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia'
});

const earlyContent = [
  '1834年：第一辆电动车诞生（Thomas Davenport）',
  '1899年：La Jamais Contente 电动车创下速度纪录（105.88 km/h）',
  '1900年：美国电动车市场份额达38%，超过蒸汽和汽油车',
  '1912年：电动启动机发明，汽油车变得更易使用',
  '1920年代：电动车因续航短、价格高而衰落'
];

earlyContent.forEach((item, idx) => {
  slide.addText(`• ${item}`, {
    x: 1.0, y: 1.5 + idx * 0.6, w: 8, h: 0.5,
    fontSize: 18,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: true
  });
});

slide.addText('关键因素：石油廉价、公路网络扩展、内燃机技术改进', {
  x: 1.0, y: 5.0, w: 8, h: 0.5,
  fontSize: 16,
  color: colors.secondary,
  fontFace: 'Calibri',
  italic: true
});

// 4. 沉寂期
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('沉寂期 (20世纪中期)', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia'
});

const dormantContent = [
  '1930-1960年代：电动车几乎消失，仅用于特定场景（高尔夫球车、叉车）',
  '1970年代：石油危机引发对电动车的短暂兴趣',
  '1976年：美国国会通过《电动和混合动力汽车研究、开发与示范法案》',
  '1980年代：通用汽车推出EV1概念车，但未大规模量产',
  '技术瓶颈：电池能量密度低、充电时间长、成本高'
];

dormantContent.forEach((item, idx) => {
  slide.addText(`• ${item}`, {
    x: 1.0, y: 1.5 + idx * 0.6, w: 8, h: 0.5,
    fontSize: 18,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: true
  });
});

slide.addText('电动车被视为“未来技术”，但尚未成熟', {
  x: 1.0, y: 5.5, w: 8, h: 0.5,
  fontSize: 16,
  color: colors.secondary,
  fontFace: 'Calibri',
  italic: true
});

// 5. 复苏与技术进步
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('复苏与技术进步 (20世纪末)', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia'
});

const recoveryContent = [
  '1990年代：加州零排放车辆（ZEV）法案推动电动车发展',
  '1996年：通用EV1量产，首款现代电动车',
  '1997年：丰田普锐斯（混合动力）上市，开启混动时代',
  '1999年：本田Insight上市，首款混动车型进入美国',
  '电池技术：镍氢电池（NiMH）逐步替代铅酸电池',
  '关键进展：电控系统、再生制动、轻量化材料'
];

recoveryContent.forEach((item, idx) => {
  slide.addText(`• ${item}`, {
    x: 1.0, y: 1.5 + idx * 0.5, w: 8, h: 0.5,
    fontSize: 18,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: true
  });
});

slide.addText('混合动力成为过渡方案，为纯电积累技术', {
  x: 1.0, y: 5.5, w: 8, h: 0.5,
  fontSize: 16,
  color: colors.secondary,
  fontFace: 'Calibri',
  italic: true
});

// 6. 21世纪爆发
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('21世纪爆发 (特斯拉引领)', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia'
});

const boomContent = [
  '2003年：特斯拉成立，专注于高性能电动车',
  '2008年：特斯拉Roadster上市，续航320公里，颠覆认知',
  '2010年：日产Leaf上市，首款大众市场纯电动车',
  '2012年：特斯拉Model S上市，获年度汽车奖',
  '2015年：全球电动车销量突破100万辆',
  '2017年：特斯拉Model 3量产，推动电动车普及',
  '2019年：全球电动车销量达220万辆',
  '电池成本十年下降87%（2010-2020）'
];

boomContent.forEach((item, idx) => {
  slide.addText(`• ${item}`, {
    x: 1.0, y: 1.5 + idx * 0.5, w: 8, h: 0.5,
    fontSize: 18,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: true
  });
});

slide.addText('特斯拉带动整个行业转向电动化', {
  x: 1.0, y: 6.0, w: 8, h: 0.5,
  fontSize: 16,
  color: colors.secondary,
  fontFace: 'Calibri',
  italic: true
});

// 7. 中国新能源汽车发展
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('中国新能源汽车发展', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia'
});

const chinaContent = [
  '2009年：“十城千辆”示范工程启动',
  '2012年：国务院发布节能与新能源汽车产业发展规划',
  '2015年：中国成为全球最大新能源汽车市场',
  '2017年：双积分政策实施，推动车企电动化',
  '2020年：新能源汽车补贴政策逐步退坡',
  '2021年：中国新能源汽车销量352万辆，占全球50%',
  '代表企业：比亚迪、蔚来、小鹏、理想等',
  '电池巨头：宁德时代、比亚迪（刀片电池）'
];

chinaContent.forEach((item, idx) => {
  slide.addText(`• ${item}`, {
    x: 1.0, y: 1.5 + idx * 0.5, w: 8, h: 0.5,
    fontSize: 18,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: true
  });
});

slide.addText('中国在电动车产业链、市场、政策方面全球领先', {
  x: 1.0, y: 6.5, w: 8, h: 0.5,
  fontSize: 16,
  color: colors.secondary,
  fontFace: 'Calibri',
  italic: true
});

// 8. 当前趋势与未来展望
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('当前趋势与未来展望', {
  x: 0.5, y: 0.5, w: 9, h: 0.8,
  fontSize: 36,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia'
});

const futureContent = [
  '电动化：全球车企宣布燃油车停产时间表（2030-2040）',
  '智能化：自动驾驶、车联网与电动车深度融合',
  '电池技术：固态电池、钠离子电池、快充技术突破',
  '能源生态：V2G（车到电网）、可再生能源整合',
  '市场规模：预计2030年电动车销量达3000万辆',
  '政策推动：各国碳中和目标加速电动化进程',
  '挑战：充电基础设施、电池回收、电网负荷'
];

futureContent.forEach((item, idx) => {
  slide.addText(`• ${item}`, {
    x: 1.0, y: 1.5 + idx * 0.5, w: 8, h: 0.5,
    fontSize: 18,
    color: colors.dark,
    fontFace: 'Calibri',
    bullet: true
  });
});

slide.addText('新能源汽车正重新定义交通与能源体系', {
  x: 1.0, y: 6.5, w: 8, h: 0.5,
  fontSize: 16,
  color: colors.secondary,
  fontFace: 'Calibri',
  italic: true
});

// 9. 结束页
slide = pres.addSlide({ masterName: 'MASTER' });
slide.addText('谢谢观看', {
  x: 0.5, y: 2.5, w: 9, h: 1.0,
  fontSize: 48,
  bold: true,
  color: colors.primary,
  fontFace: 'Georgia',
  align: 'center'
});
slide.addText('新能源汽车发展历史', {
  x: 0.5, y: 3.8, w: 9, h: 0.6,
  fontSize: 24,
  color: colors.secondary,
  fontFace: 'Calibri',
  align: 'center'
});
slide.addText('Q & A', {
  x: 0.5, y: 5.0, w: 9, h: 0.8,
  fontSize: 36,
  color: colors.accent,
  fontFace: 'Georgia',
  align: 'center',
  bold: true
});

// 保存文件
const filename = '新能源汽车发展历史.pptx';
pres.writeFile({ fileName: filename })
  .then(() => {
    console.log(`PPT 已生成: ${filename}`);
  })
  .catch(err => {
    console.error('生成PPT时出错:', err);
  });