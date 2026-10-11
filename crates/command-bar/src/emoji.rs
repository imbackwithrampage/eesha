// Emoji the bar offers: the glyph, its Unicode name, and words people look for it by.
// A few hundred in everyday use, not the whole of Unicode.
// debt: generate the full table from Unicode's emoji-test.txt (credit it in NOTICE.md) and add skin tones.

pub const EMOJI: &str = "\
😀	grinning face	smile happy\n\
😃	grinning face with big eyes	smile happy joy\n\
😄	grinning face with smiling eyes	smile happy laugh\n\
😁	beaming face	grin smile happy\n\
😆	grinning squinting face	laugh haha\n\
😅	grinning face with sweat	relief phew nervous\n\
🤣	rolling on the floor laughing	rofl lol haha\n\
😂	face with tears of joy	laugh lol cry haha\n\
🙂	slightly smiling face	smile ok\n\
🙃	upside-down face	sarcasm silly\n\
😉	winking face	wink\n\
😊	smiling face with smiling eyes	blush happy thanks\n\
😇	smiling face with halo	angel innocent\n\
🥰	smiling face with hearts	love adore\n\
😍	smiling face with heart-eyes	love crush\n\
🤩	star-struck	wow amazing excited\n\
😘	face blowing a kiss	kiss love\n\
😋	face savoring food	yum delicious tasty\n\
😛	face with tongue	tongue playful\n\
😜	winking face with tongue	joke silly\n\
🤪	zany face	crazy goofy\n\
🤑	money-mouth face	rich money\n\
🤗	hugging face	hug thanks\n\
🤭	face with hand over mouth	oops giggle\n\
🤫	shushing face	quiet secret\n\
🤔	thinking face	hmm think wonder\n\
🤐	zipper-mouth face	secret quiet\n\
🤨	face with raised eyebrow	skeptical doubt\n\
😐	neutral face	meh blank\n\
😑	expressionless face	blank\n\
😶	face without mouth	speechless silent\n\
😏	smirking face	smirk smug\n\
😒	unamused face	meh annoyed\n\
🙄	face with rolling eyes	eyeroll whatever\n\
😬	grimacing face	awkward yikes\n\
😌	relieved face	calm relief\n\
😔	pensive face	sad thoughtful\n\
😪	sleepy face	tired\n\
😴	sleeping face	sleep zzz tired\n\
😷	face with medical mask	sick ill mask\n\
🤒	face with thermometer	sick fever ill\n\
🤕	face with head-bandage	hurt injured\n\
🤢	nauseated face	sick gross\n\
🤮	face vomiting	sick puke\n\
🤧	sneezing face	cold sick achoo\n\
🥵	hot face	heat sweat\n\
🥶	cold face	freezing ice\n\
🥴	woozy face	dizzy drunk\n\
😵	face with crossed-out eyes	dizzy dead\n\
🤯	exploding head	mind blown shocked\n\
🤠	cowboy hat face	cowboy yeehaw\n\
🥳	partying face	party celebrate birthday\n\
😎	smiling face with sunglasses	cool\n\
🤓	nerd face	geek glasses\n\
🧐	face with monocle	inspect curious\n\
😕	confused face	puzzled unsure\n\
😟	worried face	concern anxious\n\
🙁	slightly frowning face	sad\n\
😮	face with open mouth	wow surprised oh\n\
😯	hushed face	surprised\n\
😲	astonished face	shocked wow\n\
😳	flushed face	embarrassed blush\n\
🥺	pleading face	please puppy eyes beg\n\
😦	frowning face with open mouth	shock\n\
😨	fearful face	scared afraid\n\
😰	anxious face with sweat	nervous stress\n\
😥	sad but relieved face	phew\n\
😢	crying face	sad tear\n\
😭	loudly crying face	sob cry sad tears\n\
😱	face screaming in fear	scream scared omg\n\
😖	confounded face	frustrated\n\
😣	persevering face	struggle\n\
😞	disappointed face	sad let down\n\
😓	downcast face with sweat	tired hard\n\
😩	weary face	tired exhausted\n\
😫	tired face	exhausted fed up\n\
🥱	yawning face	tired bored sleepy\n\
😤	face with steam from nose	frustrated huff triumph\n\
😡	enraged face	angry mad rage\n\
😠	angry face	mad annoyed\n\
🤬	face with symbols on mouth	swearing cursing angry\n\
😈	smiling face with horns	devil mischief\n\
💀	skull	dead death lol\n\
💩	pile of poo	poop crap\n\
🤡	clown face	clown joke\n\
👻	ghost	halloween boo spooky\n\
👽	alien	ufo space\n\
🤖	robot	bot ai machine\n\
😺	grinning cat	cat smile\n\
🙈	see-no-evil monkey	monkey hide shy\n\
🙉	hear-no-evil monkey	monkey\n\
🙊	speak-no-evil monkey	monkey secret oops\n\
❤️	red heart	love heart\n\
🧡	orange heart	love heart\n\
💛	yellow heart	love heart\n\
💚	green heart	love heart\n\
💙	blue heart	love heart\n\
💜	purple heart	love heart\n\
🖤	black heart	love heart dark\n\
🤍	white heart	love heart\n\
💔	broken heart	heartbreak sad\n\
💕	two hearts	love\n\
💖	sparkling heart	love\n\
💯	hundred points	100 perfect score\n\
💥	collision	boom bang explosion\n\
💫	dizzy	stars\n\
💦	sweat droplets	water splash\n\
💤	zzz	sleep tired\n\
👋	waving hand	hello hi bye wave\n\
🤚	raised back of hand	stop\n\
✋	raised hand	stop high five\n\
🖖	vulcan salute	spock\n\
👌	ok hand	okay perfect\n\
🤏	pinching hand	small tiny\n\
✌️	victory hand	peace two\n\
🤞	crossed fingers	luck hope\n\
🤟	love-you gesture	ily\n\
🤘	sign of the horns	rock metal\n\
🤙	call me hand	shaka phone\n\
👈	backhand index pointing left	left point\n\
👉	backhand index pointing right	right point\n\
👆	backhand index pointing up	up point\n\
👇	backhand index pointing down	down point\n\
☝️	index pointing up	one wait\n\
👍	thumbs up	like yes good ok approve +1\n\
👎	thumbs down	dislike no bad -1\n\
✊	raised fist	power solidarity\n\
👊	oncoming fist	punch bump\n\
👏	clapping hands	applause bravo congrats\n\
🙌	raising hands	hooray celebrate praise\n\
👐	open hands	hug\n\
🤲	palms up together	pray offer\n\
🤝	handshake	deal agree\n\
🙏	folded hands	thanks please pray\n\
✍️	writing hand	write sign\n\
💪	flexed biceps	strong muscle power\n\
👀	eyes	look see watching\n\
👁️	eye	look see\n\
🧠	brain	smart think mind\n\
👶	baby	child infant\n\
🧑	person	human adult\n\
👨‍💻	man technologist	developer coder programmer laptop\n\
👩‍💻	woman technologist	developer coder programmer laptop\n\
🤷	person shrugging	shrug dunno whatever\n\
🤦	person facepalming	facepalm doh\n\
🙋	person raising hand	question me\n\
🏃	person running	run hurry fast\n\
🚶	person walking	walk\n\
🐶	dog face	dog puppy pet\n\
🐱	cat face	cat kitten pet\n\
🐭	mouse face	mouse\n\
🐹	hamster	pet\n\
🐰	rabbit face	bunny\n\
🦊	fox	animal\n\
🐻	bear	animal\n\
🐼	panda	animal\n\
🐨	koala	animal\n\
🐯	tiger face	animal\n\
🦁	lion	animal\n\
🐮	cow face	animal\n\
🐷	pig face	animal\n\
🐸	frog	animal\n\
🐵	monkey face	animal\n\
🐔	chicken	bird hen\n\
🐧	penguin	bird linux\n\
🐦	bird	tweet\n\
🦆	duck	bird\n\
🦉	owl	bird wise\n\
🐴	horse face	animal\n\
🦄	unicorn	magic\n\
🐝	honeybee	bee\n\
🐛	bug	insect defect\n\
🦋	butterfly	insect\n\
🐌	snail	slow\n\
🐢	turtle	slow\n\
🐍	snake	python\n\
🐙	octopus	sea\n\
🐠	tropical fish	fish\n\
🐬	dolphin	sea\n\
🐳	spouting whale	whale docker\n\
🦀	crab	rust\n\
🌵	cactus	plant desert\n\
🎄	christmas tree	xmas holiday\n\
🌲	evergreen tree	tree forest\n\
🌴	palm tree	beach holiday\n\
🌱	seedling	plant grow sprout\n\
🍀	four leaf clover	luck\n\
🌹	rose	flower\n\
🌻	sunflower	flower\n\
🌸	cherry blossom	flower spring\n\
🍁	maple leaf	autumn fall canada\n\
🍎	red apple	fruit\n\
🍊	tangerine	orange fruit\n\
🍋	lemon	fruit\n\
🍌	banana	fruit\n\
🍉	watermelon	fruit\n\
🍇	grapes	fruit\n\
🍓	strawberry	fruit\n\
🍒	cherries	fruit\n\
🍑	peach	fruit\n\
🥑	avocado	fruit\n\
🍅	tomato	vegetable\n\
🥕	carrot	vegetable\n\
🌽	ear of corn	maize vegetable\n\
🌶️	hot pepper	chili spicy\n\
🍞	bread	toast loaf\n\
🧀	cheese wedge	cheese\n\
🍳	cooking	egg fried breakfast\n\
🥓	bacon	breakfast\n\
🍔	hamburger	burger food\n\
🍟	french fries	chips food\n\
🍕	pizza	food slice\n\
🌭	hot dog	food\n\
🌮	taco	food\n\
🌯	burrito	food\n\
🍝	spaghetti	pasta food\n\
🍜	steaming bowl	ramen noodles soup\n\
🍣	sushi	food japanese\n\
🍦	soft ice cream	dessert\n\
🍩	doughnut	donut dessert\n\
🍪	cookie	biscuit dessert\n\
🎂	birthday cake	cake party\n\
🍰	shortcake	cake dessert\n\
🍫	chocolate bar	sweet\n\
🍬	candy	sweet\n\
🍿	popcorn	movie snack\n\
☕	hot beverage	coffee tea\n\
🍵	teacup without handle	tea\n\
🥤	cup with straw	soda drink\n\
🍺	beer mug	beer drink\n\
🍻	clinking beer mugs	cheers beer\n\
🥂	clinking glasses	cheers toast champagne\n\
🍷	wine glass	wine drink\n\
🍸	cocktail glass	drink\n\
🍾	bottle with popping cork	champagne celebrate\n\
⚽	soccer ball	football sport\n\
🏀	basketball	sport\n\
🏈	american football	sport\n\
⚾	baseball	sport\n\
🎾	tennis	sport\n\
🏐	volleyball	sport\n\
🎱	pool 8 ball	billiards\n\
🏆	trophy	win award champion\n\
🥇	1st place medal	gold first winner\n\
🥈	2nd place medal	silver second\n\
🥉	3rd place medal	bronze third\n\
🏅	sports medal	award\n\
🎯	bullseye	target goal dart\n\
🎮	video game	gaming controller\n\
🎲	game die	dice random\n\
🧩	puzzle piece	jigsaw\n\
🎸	guitar	music\n\
🎹	musical keyboard	piano music\n\
🎧	headphone	music audio\n\
🎤	microphone	sing karaoke\n\
🎬	clapper board	movie film\n\
🎨	artist palette	art paint design\n\
🚗	automobile	car drive\n\
🚕	taxi	cab\n\
🚌	bus	transport\n\
🚲	bicycle	bike cycle\n\
✈️	airplane	plane flight travel\n\
🚀	rocket	launch ship fast deploy\n\
🚁	helicopter	fly\n\
🚂	locomotive	train\n\
🚢	ship	boat cruise\n\
⚓	anchor	sea\n\
🏠	house	home\n\
🏢	office building	work\n\
🏥	hospital	health\n\
🏫	school	education\n\
🗽	statue of liberty	new york\n\
🌍	globe showing europe-africa	earth world\n\
🌎	globe showing americas	earth world\n\
🌏	globe showing asia-australia	earth world\n\
🌙	crescent moon	night\n\
⭐	star	favorite\n\
🌟	glowing star	shine\n\
✨	sparkles	shiny new magic\n\
⚡	high voltage	lightning zap fast power\n\
🔥	fire	hot lit flame\n\
💧	droplet	water\n\
🌈	rainbow	pride colors\n\
☀️	sun	sunny weather\n\
⛅	sun behind cloud	cloudy weather\n\
☁️	cloud	weather\n\
🌧️	cloud with rain	rain weather\n\
⛈️	cloud with lightning and rain	storm thunder\n\
❄️	snowflake	snow cold winter\n\
☃️	snowman	snow winter\n\
🌊	water wave	sea ocean surf\n\
🎉	party popper	celebrate congrats tada\n\
🎊	confetti ball	celebrate party\n\
🎈	balloon	party birthday\n\
🎁	wrapped gift	present birthday\n\
🎀	ribbon	bow\n\
⌚	watch	time\n\
📱	mobile phone	smartphone iphone\n\
💻	laptop	computer pc\n\
⌨️	keyboard	type\n\
🖥️	desktop computer	pc monitor screen\n\
🖨️	printer	print\n\
🖱️	computer mouse	click\n\
💾	floppy disk	save\n\
💿	optical disk	cd dvd\n\
📷	camera	photo picture\n\
🎥	movie camera	video film\n\
📺	television	tv\n\
📻	radio	music\n\
🔋	battery	power charge\n\
🔌	electric plug	power\n\
💡	light bulb	idea\n\
🔦	flashlight	torch light\n\
📖	open book	read\n\
📚	books	library study\n\
📰	newspaper	news\n\
💰	money bag	rich cash\n\
💵	dollar banknote	money cash\n\
💳	credit card	pay money\n\
✉️	envelope	mail letter email\n\
📧	e-mail	mail email\n\
📦	package	box parcel ship\n\
📝	memo	note write\n\
📁	file folder	directory\n\
📂	open file folder	directory\n\
📅	calendar	date schedule\n\
📈	chart increasing	graph up growth\n\
📉	chart decreasing	graph down\n\
📊	bar chart	graph stats\n\
📌	pushpin	pin\n\
📎	paperclip	attach\n\
✂️	scissors	cut\n\
🔒	locked	lock secure\n\
🔓	unlocked	unlock open\n\
🔑	key	password\n\
🔨	hammer	tool build\n\
🛠️	hammer and wrench	tools fix build\n\
🔧	wrench	tool fix\n\
⚙️	gear	settings cog\n\
🧪	test tube	test experiment science\n\
🧲	magnet	attract\n\
🔬	microscope	science\n\
🔭	telescope	space\n\
💊	pill	medicine\n\
💉	syringe	vaccine shot\n\
🧹	broom	clean sweep\n\
🗑️	wastebasket	trash delete bin\n\
🔔	bell	notification alert\n\
🔕	bell with slash	mute silent\n\
📣	megaphone	announce\n\
🔍	magnifying glass tilted left	search find\n\
🔗	link	url chain\n\
⏰	alarm clock	time wake\n\
⏳	hourglass not done	wait time loading\n\
⌛	hourglass done	time\n\
⏱️	stopwatch	timer\n\
✅	check mark button	done yes ok tick\n\
☑️	check box with check	done tick\n\
✔️	check mark	done yes tick\n\
❌	cross mark	no wrong x delete\n\
❎	cross mark button	no x\n\
❓	red question mark	question what\n\
❗	red exclamation mark	important warning\n\
⚠️	warning	caution alert\n\
🚫	prohibited	no forbidden ban\n\
⛔	no entry	stop forbidden\n\
🚧	construction	wip work in progress\n\
♻️	recycling symbol	recycle\n\
➕	plus	add\n\
➖	minus	subtract remove\n\
➡️	right arrow	next forward\n\
⬅️	left arrow	back previous\n\
⬆️	up arrow	up\n\
⬇️	down arrow	down\n\
🔄	counterclockwise arrows button	refresh sync reload\n\
🔴	red circle	dot\n\
🟠	orange circle	dot\n\
🟡	yellow circle	dot\n\
🟢	green circle	dot\n\
🔵	blue circle	dot\n\
🟣	purple circle	dot\n\
⚫	black circle	dot\n\
⚪	white circle	dot\n\
🏁	chequered flag	finish race done\n\
🚩	triangular flag	red flag warning";
