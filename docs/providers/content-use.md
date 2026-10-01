# Content use

Some vendors say that what you send them, and what their models answer, may be
used to train or improve their models. On those routes crucible asks you once
before anything is sent, and remembers your answer.

## What is asked, and when

The first time a message would go on such a route, a panel stands in place of
the prompt box. It names the route, says what the vendor's own terms say in
English, and names the page and the day it was read. Nothing has been sent at
that point: not the message, not a web search, not a sign-in, not the renewal
of a token.

- **Use it anyway** sends the message and writes the route into your own
  configuration file, so the question is not asked again, in this run or a
  later one.
- **Go back**, or Escape, sends nothing and leaves your message in the prompt
  box.

The same question is asked when you choose such a route in `/login`, or a
model of one in `/model`, before the choice is taken. A yes given in `/login`
is written down once the credential is stored; a sign-in that fails or is
left writes nothing down.

A window too short for the whole panel says so and sends nothing:

```text
! this route needs an answer first; make the window taller and send again
```

A run with no terminal to ask on ends before sending, with what the vendor's
terms say and `Nothing was sent; answer it once in a terminal.` on standard
error, and exit status 1. A client of the application with no terminal is put
the same question as a pending `warning`, answered with an `accepted` or a
`declined` decision naming it.

Behind the question, every request crucible sends to a host of such a route is
held until the route has its yes, however the route was reached: `/login`,
`/model`, a key in the environment, the configuration file, `--model`, or a
session picked up with `--continue` or `--resume`.

## Where the answer is kept

Each yes is a route name in `contentUse.accepted`, in the configuration file
in your home directory:

```json
{ "contentUse": { "accepted": ["key:google"] } }
```

Only that file is read for it. Either project file that sets
`contentUse.accepted` stops crucible before it draws anything, naming the file
and the key, as every key a checkout may not set does.

A yes is taken out of the file before a credential of its route is removed or
replaced by one of another row: `/logout`, a sign-in or key on another row of
the same provider, and a second credential taken out at a start. With it go
the yes of every model of that provider and the yes of the route its
`baseUrl` answers for. A new account on the route has agreed to nothing, so it
is asked again. A key changed in the environment moves nothing. To take a yes
back yourself, delete its name from the list.

## The routes asked about

A route is a `/login` row, named by its list and the name its credential is
stored under; a model that is itself asked about, named `model:`, its provider,
`/` and the model; or a Kimi open platform address, named by its host. The
sentence is what the panel says; the caution is what the row says in `/login`.
Each quote is the vendor's own, in the language it was written in.

| Route | Name | Condition | Panel sentence | Caution | Vendor's words | Way out | Source | Read |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OpenAI subscription | `subscription:openai` | On Free, Plus and Pro | On Free, Plus and Pro, OpenAI may use what you send to train its models. To stop it, turn off Improve the model for everyone under Settings, Data controls in ChatGPT, or choose Do not train on my content in its Privacy Portal; that covers only what you send afterwards. | may train on what is sent | "When you use our services for individuals, such as ChatGPT and Codex, we may use your content to train our models. [...] To opt out, turn off Improve the model for everyone under Settings > Data controls in ChatGPT, or select Do not train on my content in our Privacy Portal." "If you are on a ChatGPT Plus, ChatGPT Pro or ChatGPT Free plan on a personal workspace, data sharing is enabled for you by default" | Settings, Data controls in ChatGPT, or the Privacy Portal | [OpenAI Help Center, archived copy](https://help.openai.com/en/articles/5722486-how-your-data-is-used-to-improve-model-performance) | 28 Sep 2026 |
| Kimi Code · kimi.ai subscription | `subscription:moonshot@kimi.ai` | | Kimi may use what you send to train its models. To stop it, contact Kimi as its terms say; that covers only what you send afterwards. | may train on what is sent | "subject to your training opt-out below, use Content to train, evaluate, and improve the Services." "You can opt out of allowing your Content to be used to train the Services by contacting us [...] Opt-out applies prospectively only" | Contact Kimi, for what is sent afterwards | [kimi.ai terms of service](https://www.kimi.ai/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| Kimi Code · kimi.com subscription | `subscription:moonshot` | | Kimi may use what you send, and what it answers, to improve its models. To keep it out of training, contact Kimi as its terms say. | may use what is sent | "为了提升您使用本服务的体验，您授予我们一项免费的使用权，以在法律允许的范围内将您输入输出之内容及反馈用于模型服务优化。如您不希望您的内容被用于模型训练，您可以通过本协议所载的联系方式联系我们。" | Contact Kimi | [kimi.com user agreement](https://www.kimi.com/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| MiniMax Token Plan · minimax.io subscription | `subscription:minimax@token-plan.minimax.io` | | MiniMax may use what you send, and what it answers, to develop and improve its services. | may use what is sent | "We may use the input and generated content to provide, maintain, develop, and improve our Services, comply with applicable law, enforce our terms and policies, and keep our Services safe." | None given | [MiniMax terms of service](https://platform.minimax.io/protocol/terms-of-service) | 1 Oct 2026 |
| MiniMax Token Plan · minimaxi.com subscription | `subscription:minimax@token-plan.minimaxi.com` | | MiniMax may use what you send, and what it answers, once de-identified, to optimise its services. | may use what is sent | "您知悉并授权，在经安全加密技术处理、去标识化且无法重新识别特定个人的前提下，我们可能会将服务所收集的输入及对应输出，用于本协议下服务的优化以及统计分析、问题排查、安全风控等目的。" | None given | [MiniMax user agreement, minimaxi.com](https://platform.minimaxi.com/protocol/user-agreement) | 1 Oct 2026 |
| Qwen Coding Plan · aliyun.com subscription | `subscription:qwen@coding-plan.aliyun.com` | | Alibaba Cloud uses what you send, and what the model answers, to improve its service and its models. To stop it, stop using the Coding Plan; that covers only what you send afterwards. | uses what is sent | "数据使用授权：使用 Coding Plan 期间，模型输入以及模型生成的内容将用于服务改进与模型优化。停止使用 Coding Plan 服务可终止后续数据授权，但终止授权的范围不涵盖已授权使用的 Coding Plan 数据。" | Stop using the plan, for what is sent afterwards | [Qwen Coding Plan, aliyun.com](https://help.aliyun.com/zh/model-studio/coding-plan) | 1 Oct 2026 |
| Qwen Token Plan · aliyun.com subscription | `subscription:qwen@token-plan.aliyun.com` | On the Personal edition | On the Personal edition, Alibaba Cloud uses what you send, and what the model answers, to improve its service and its models. To stop it, stop using the plan; that covers only what you send afterwards. | uses what is sent | "数据使用授权：使用 Token Plan 个人版期间，模型输入以及模型生成的内容将用于服务改进与模型优化。停止使用 Token Plan 个人版服务可终止后续数据授权，但终止授权的范围不涵盖已授权使用的数据。" | Stop using the plan, for what is sent afterwards | [Qwen Token Plan Personal, aliyun.com](https://help.aliyun.com/zh/model-studio/token-plan-personal-overview) | 1 Oct 2026 |
| Google key | `key:google` | On unpaid quota | On unpaid quota, Google uses what you send and what it answers to improve its products and machine learning technologies. In the EEA, Switzerland and the UK the paid terms apply instead. | uses what is sent | "When you use Unpaid Services, including, for example, Google AI Studio and the unpaid quota on Gemini API, Google uses the content you submit to the Services and any generated responses to provide, improve, and develop Google products and services and machine learning technologies" "If you're in the European Economic Area, Switzerland, or the United Kingdom, the terms under "How Google uses Your Data" in "Paid Services" apply to all Services, including Google AI Studio and unpaid quota in the Gemini API" | None given | [Gemini API terms](https://ai.google.dev/gemini-api/terms) | 30 Sep 2026 |
| MiniMax · minimax.io key | `key:minimax@minimax.io` | | MiniMax may use what you send, and what it answers, to develop and improve its services. | may use what is sent | As the minimax.io subscription: the same terms cover a pay-as-you-go key and a plan. | None given | [MiniMax terms of service](https://platform.minimax.io/protocol/terms-of-service) | 1 Oct 2026 |
| MiniMax · minimaxi.com key | `key:minimax@minimaxi.com` | | MiniMax may use what you send, and what it answers, once de-identified, to optimise its services. | may use what is sent | As the minimaxi.com subscription: the same agreement covers a pay-as-you-go key and a plan. | None given | [MiniMax user agreement, minimaxi.com](https://platform.minimaxi.com/protocol/user-agreement) | 1 Oct 2026 |
| MoonshotAI · kimi.ai key | `key:moonshot@kimi.ai` | | Kimi may use what you send to train its models. To stop it, contact Kimi as its terms say; that covers only what you send afterwards. | may train on what is sent | As the kimi.ai subscription: a Kimi Code Console key is the same service. | Contact Kimi, for what is sent afterwards | [kimi.ai terms of service](https://www.kimi.ai/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| MoonshotAI · kimi.com key | `key:moonshot` | | Kimi may use what you send, and what it answers, to improve its models. To keep it out of training, contact Kimi as its terms say. | may use what is sent | As the kimi.com subscription: a Kimi Code Console key is the same service. | Contact Kimi | [kimi.com user agreement](https://www.kimi.com/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| Z.ai · bigmodel.cn key | `key:zai@bigmodel.cn` | | Zhipu may use what you send, once anonymised, to improve its products and services, including to train its models, without asking you again. | may train on what is sent | "若我们对您的内容采取技术措施和其他必要措施进行处理，使得数据接收方无法重新识别特定个人且不能复原，[...] 以及改进我们的产品和服务（包括使用匿名数据进行机器学习或模型算法训练），按照相关法律法规规定，此类数据已不属于个人信息范畴，因此此类处理后的数据的使用无需另行征得您的同意。" | None given | [bigmodel.cn user agreement](https://docs.bigmodel.cn/cn/terms/user-agreement) | 1 Oct 2026 |
| Meta `muse-spark-1.3-contributor` | `model:meta/muse-spark-1.3-contributor` | | Meta may use what you send to train, develop, evaluate and improve its AI models, products and services, and gives no way to keep it out of training. Its terms say not to send code or anything else you must keep confidential. | may train on what is sent | "You agree that Meta may use your Content to train, develop, evaluate, and improve Meta's artificial intelligence models, products, and services." "the Discounted Services do not offer a mechanism to exclude specific traffic from training." "If you intend, or are required (including by contract), to keep information such as software code confidential, you must not submit that information to the Discounted Services." | None; Meta does not train on the standard models | [Meta Model API terms of service](https://dev.meta.ai/legal/terms-of-service) | 1 Oct 2026 |
| Meta `muse-spark-1.2-contributor` | `model:meta/muse-spark-1.2-contributor` | | Meta may use what you send to train, develop, evaluate and improve its AI models, products and services, and gives no way to keep it out of training. Its terms say not to send code or anything else you must keep confidential. | may train on what is sent | As `muse-spark-1.3-contributor`: Meta's terms name both as Discounted Models. | None; Meta does not train on the standard models | [Meta Model API terms of service](https://dev.meta.ai/legal/terms-of-service) | 1 Oct 2026 |
| Kimi open platform, `api.moonshot.ai` | `api.moonshot.ai` | | Moonshot may use what you send to develop and improve its services, and to train its models, unless you agree otherwise with it in writing. | may train on what is sent | "We may use Content to provide, maintain, develop, support, and improve the Services [...] Customer who requires restrictions on the use of Customer Content for training or improving Moonshot AI models may contact Moonshot AI to discuss available enterprise arrangements or separate written agreements. Unless otherwise expressly agreed in writing, Customer Content may be used for the foregoing purposes." | A written agreement with Moonshot | [Kimi open platform terms](https://platform.kimi.ai/docs/agreement/modeluse) | 30 Sep 2026 |
| Kimi open platform, `api.moonshot.cn` | `api.moonshot.cn` | | Moonshot may use what you send, and what it answers, to improve its models. | may use what is sent | "为了提升您使用本服务的体验，您授予我们一项免费的使用权，以在法律允许的范围内将您输入输出之内容及反馈用于模型服务优化。" | None given | [Kimi open platform terms, platform.kimi.com](https://platform.kimi.com/docs/agreement/modeluse) | 30 Sep 2026 |

The OpenAI help pages refuse automated readers, so that row rests on the
Internet Archive's copies of 28 and 14 September 2026, the date shown being the
later.

The two Meta contributor models are asked about when one is chosen in
`/model`, and at the first send on either, whichever way it was reached; the
yes is kept for each model on its own. The Meta key row and the standard
models, `muse-spark-1.3` and `muse-spark-1.2`, are not asked about: Meta's
terms say it does not use what is sent to them to train its models. A web
search names the model the session is asking now, so it is held while that
model is a contributor model without its yes, and goes out on the standard
model once the session has moved to one.

The Qwen rows of aliyun.com rest on the plan pages of that site, read in
Chinese; Alibaba Cloud's service agreement says the same of both plans. The
Token Plan Team edition is not trained on, but its key cannot be told from a
Personal one, so that row states the edition first.

These are not asked about, because their vendors' terms say what is sent is
not used for training unless you agree:

- the `OpenAI` and `Anthropic` key rows;
- `xAI`: "SpaceXAI never trains on your API inputs or outputs without your
  explicit permission";
- `MiMo`: Xiaomi's privacy policies for the platform say it does not use what
  you send for model training: the one outside mainland China outright, the
  mainland one without your prior consent;
- `Z.ai · z.ai`: "We will not use End User Content to develop or improve
  Services, unless you explicitly agree to such use";
- `Qwen · alibabacloud.com`, `Qwen · aliyun.com`, and the Qwen Coding Plan and
  Token Plan rows of alibabacloud.com: Alibaba Cloud's terms on both sites say
  it does not use what is sent to train its models without your consent, and
  the alibabacloud.com plan pages add nothing to that.

`DeepSeek` is not asked about either: its open platform terms say nothing of
training on what is sent through the API, either way. Its general terms of use
say it may use inputs and outputs to improve its services, and do not say
whether they reach the API.

A key from a provider's variable belongs to one row of it: for MoonshotAI, the
kimi.com key row; for MiniMax, the minimax.io key row, which is asked about; for
Qwen, the alibabacloud.com key row; for Z.ai, the z.ai key row.

## A `baseUrl` crucible recognises

A `baseUrl` is warned only at an address crucible documents. Where two rows
whose credential is a key send to one address, as a MiniMax plan and a MiniMax
key do, the address answers for the `API key` row. It is recognised
when its scheme is the address's, its host is the address's (case, a trailing
dot and the default port written out do not matter), and its path is the
address's path or lies under it by whole segments, under any of six
readings: with adjacent slashes merged or kept, and for each, `.` and `..`
resolved before percent-encoding is decoded, or after it with an encoded `/`
kept inside its segment, or after it with an encoded `/` taken as a separator.
Any one of those readings being documented is enough to be asked. A path
written with `\` for `/`, or with `;` parameters, is not read as the
documented path. Every other `baseUrl`, including
`https://chatgpt.com/backend-api/codex/responses`, where no key row stands, is
not asked about.

| Address | Route it answers for |
| --- | --- |
| `https://api.anthropic.com/v1` | `key:anthropic`, not asked about |
| `https://api.deepseek.com` | `key:deepseek@deepseek.com`, not asked about |
| `https://generativelanguage.googleapis.com/v1beta` | `key:google` |
| `https://api.kimi.ai/coding/v1` | `key:moonshot@kimi.ai` |
| `https://api.kimi.com/coding/v1` | `key:moonshot` |
| `https://api.meta.ai/v1` | `key:meta@meta.ai`, not asked about; a contributor model is still asked about |
| `https://api.xiaomimimo.com/v1` | `key:mimo@xiaomimimo.com`, not asked about |
| `https://api.minimax.io/v1` | `key:minimax@minimax.io`; the minimax.io plan sends here too |
| `https://api.minimax.cn/v1` | `key:minimax@minimaxi.com`; the minimaxi.com plan sends here too |
| `https://api.openai.com/v1` | `key:openai`, not asked about |
| `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | `key:qwen@alibabacloud.com`, not asked about |
| `https://dashscope.aliyuncs.com/compatible-mode/v1` | `key:qwen@aliyun.com`, not asked about |
| `https://coding-intl.dashscope.aliyuncs.com/v1` | `subscription:qwen@coding-plan.alibabacloud.com`, not asked about |
| `https://coding.dashscope.aliyuncs.com/v1` | `subscription:qwen@coding-plan.aliyun.com` |
| `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` | `subscription:qwen@token-plan.alibabacloud.com`, not asked about |
| `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | `subscription:qwen@token-plan.aliyun.com` |
| `https://api.x.ai/v1` | `key:xai@x.ai`, not asked about |
| `https://api.z.ai/api/paas/v4` | `key:zai@z.ai`, not asked about |
| `https://open.bigmodel.cn/api/paas/v4` | `key:zai@bigmodel.cn` |
| `https://api.moonshot.ai/v1` | `api.moonshot.ai` |
| `https://api.moonshot.cn/v1` | `api.moonshot.cn` |

## Rolling back to 0.43.3

0.43.3 does not know `contentUse`, and stops before drawing anything on a file
that has it, with a line that begins:

```text
crucible: <home>/config.json: contentUse is not a setting crucible has at line <n>, column <m>
```

Delete the `contentUse` block from the configuration file in your home
directory before running 0.43.3.
